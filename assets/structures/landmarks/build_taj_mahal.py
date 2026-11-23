#!/usr/bin/env python3
"""Builds taj_mahal.schem (Taj Mahal, Agra, Q9141) beside this script.

    python3 assets/structures/landmarks/build_taj_mahal.py [OUT.schem]

Needs numpy. One block per metre, x east, y up, z south. The axis of the complex runs
through the middle of block x=0; the mausoleum's centre is block (0, z=0). Layer y=0
is the first above the riverfront terrace (chameli farsh); the garden turf is layer -2.
The model runs from the river wall to the great gate: terrace, mausoleum, mosque and
jawab, corner towers, the charbagh with its walks, channels and raised tank, the
enclosure walls with the drum houses, and the gate. The forecourt beyond is left to OSM.

Plan: Koch & Barraud's survey (E. Koch, The Complete Taj Mahal, 2006, pp. 258-259):
mausoleum 56.9 m square, minarets 5.65 m wide and 43 m tall, mosque 56.6 x 23.4 m,
terrace 300 x 111.9 m, charbagh 296.3 m square, great gate 41.2 x 34 m. Positions of
the terrace buildings, towers and drum houses from OpenStreetMap (ODbL); its gate and
garden walks lie 9 m east of the axis the rest agrees on, so only their north-south
positions are used. Heights from a telephoto taken across the river (A.Savin, Wikimedia
Commons, FAL), calibrated on the minarets: plinth 5.8 m, parapet 29, pishtaq 34.6,
drum top 40.7, dome 29.3 m wide reaching 60.9, finial tip 74.2 m above the terrace.
"""
import gzip
import io
import math
import os
import struct
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
DATA_VERSION = 4438

# Model extent, inclusive.
X0, X1 = -156, 156
Z0, Z1 = -66, 390
Y0, Y1 = -12, 76


# ---------------------------------------------------------------- NBT + Sponge v3


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


def sponge(grid, names):
    """Gzipped Sponge v3 of `grid` [x, y, z] indexing `names`, cropped to what is used."""
    used = np.argwhere(grid > 0)
    lo, hi = used.min(0), used.max(0) + 1
    g = grid[lo[0]:hi[0], lo[1]:hi[1], lo[2]:hi[2]]
    ids = np.unique(g)
    remap = np.zeros(len(names), np.int64)
    palette = {}
    for i in ids:
        remap[i] = len(palette)
        palette[names[i]] = len(palette)
    w, h, l = g.shape
    # Sponge order: index = (y * length + z) * width + x.
    flat = remap[g].transpose(1, 2, 0).reshape(-1)
    blocks = {
        "Palette": (10, {k: (3, v) for k, v in palette.items()}),
        "Data": (7, _varints(flat.tolist())),
        "BlockEntities": (9, (10, [])),
    }
    schematic = {
        "Version": (3, 3),
        "DataVersion": (3, DATA_VERSION),
        "Width": (2, int(w)),
        "Height": (2, int(h)),
        "Length": (2, int(l)),
        "Offset": (11, [0, 0, 0]),
        "Blocks": (10, blocks),
    }
    out = io.BytesIO()
    out.write(b"\x0a" + struct.pack(">H", 0))
    _payload(out, 10, {"Schematic": (10, schematic)})
    return gzip.compress(out.getvalue(), mtime=0), lo


# ---------------------------------------------------------------- blocks


def state(name, **props):
    name = name if ":" in name else "minecraft:" + name
    if not props:
        return name
    inner = ",".join(f"{k}={str(v).lower()}" for k, v in sorted(props.items()))
    return f"{name}[{inner}]"


def stairs(name, facing, half="bottom", shape="straight"):
    return state(name, facing=facing, half=half, shape=shape, waterlogged=False)


def slab(name, kind="bottom"):
    return state(name, type=kind, waterlogged=False)


AIR = "minecraft:air"
MARBLE = state("smooth_quartz")
MARBLE_B = state("quartz_block")
MARBLE_BRICK = state("quartz_bricks")
MARBLE_CARVED = state("chiseled_quartz_block")
MARBLE_PILLAR = state("quartz_pillar", axis="y")
MARBLE_SLAB = slab("smooth_quartz_slab")
MARBLE_SLAB_TOP = slab("smooth_quartz_slab", "top")
DARK = state("black_concrete")
JALI = state("gray_concrete")
CALLIGRAPHY = state("diorite")
GOLD = state("gold_block")
SAND_RED = state("red_terracotta")
SAND_TRIM = state("granite")
SAND_SLAB = slab("polished_granite_slab")
PAVE_LINE = state("polished_granite")
JALI_RED = state("nether_bricks")
WATER = state("water", level=0)
GRASS = state("grass_block", snowy=False)
TUFT = state("short_grass")


def marble_stairs(facing, half="bottom", shape="straight"):
    return stairs("smooth_quartz_stairs", facing, half, shape)


def sand_stairs(facing, half="bottom", shape="straight"):
    return stairs("granite_stairs", facing, half, shape)


ROD = state("lightning_rod", facing="up", powered=False, waterlogged=False)

# Directions in model space.
OUT = {(0, 1): "south", (0, -1): "north", (1, 0): "east", (-1, 0): "west"}
OPP = {"south": "north", "north": "south", "east": "west", "west": "east"}
LEFT = {"north": "west", "west": "south", "south": "east", "east": "north"}


class Grid:
    def __init__(self):
        self.a = np.zeros((X1 - X0 + 1, Y1 - Y0 + 1, Z1 - Z0 + 1), np.uint16)
        self.names = [AIR]
        self.index = {AIR: 0}

    def id(self, name):
        i = self.index.get(name)
        if i is None:
            i = self.index[name] = len(self.names)
            self.names.append(name)
        return i

    def set(self, x, y, z, name):
        if X0 <= x <= X1 and Y0 <= y <= Y1 and Z0 <= z <= Z1:
            self.a[x - X0, y - Y0, z - Z0] = self.id(name)

    def get(self, x, y, z):
        if X0 <= x <= X1 and Y0 <= y <= Y1 and Z0 <= z <= Z1:
            return self.names[self.a[x - X0, y - Y0, z - Z0]]
        return AIR

    def fill(self, x0, x1, y0, y1, z0, z1, name):
        x0, x1 = sorted((x0, x1))
        y0, y1 = sorted((y0, y1))
        z0, z1 = sorted((z0, z1))
        self.a[x0 - X0:x1 - X0 + 1, y0 - Y0:y1 - Y0 + 1, z0 - Z0:z1 - Z0 + 1] = self.id(name)

    def column(self, x, z, y0, y1, name):
        for y in range(y0, y1 + 1):
            self.set(x, y, z, name)


# ---------------------------------------------------------------- shapes


def pointed(u, a, rise):
    """Height of a Mughal four-centred arch over its springing at offset u of half-width a:
    rounded haunches, then straight flanks meeting in a point."""
    t = min(abs(u) / a, 1.0)
    return rise * min(math.sqrt(1.0 - t), 1.0 - 0.9 * t)


def disc_cells(cx, cz, r):
    """Block cells whose centres lie within r of (cx, cz), less single-cell nubs sticking
    out of the four sides, which read as a cross."""
    out = set()
    for x in range(math.floor(cx - r) - 1, math.ceil(cx + r) + 2):
        for z in range(math.floor(cz - r) - 1, math.ceil(cz + r) + 2):
            if math.hypot(x - cx, z - cz) <= r:
                out.add((x, z))
    if len(out) > 5:
        xs = [c[0] for c in out]
        zs = [c[1] for c in out]
        for edge in ([c for c in out if c[0] == min(xs)], [c for c in out if c[0] == max(xs)],
                     [c for c in out if c[1] == min(zs)], [c for c in out if c[1] == max(zs)]):
            if len(edge) == 1:
                out.discard(edge[0])
    return sorted(out)


def octagon_cells(cx, cz, r):
    """Regular octagon of inradius r, flat sides facing the axes."""
    out = []
    for x in range(math.floor(cx - r) - 1, math.ceil(cx + r) + 2):
        for z in range(math.floor(cz - r) - 1, math.ceil(cz + r) + 2):
            dx, dz = abs(x - cx), abs(z - cz)
            if dx <= r and dz <= r and dx + dz <= r * math.sqrt(2):
                out.append((x, z))
    return out


def interp(table, h):
    if h <= table[0][0]:
        return table[0][1]
    for (h0, r0), (h1, r1) in zip(table, table[1:]):
        if h0 <= h <= h1:
            return r0 + (r1 - r0) * (h - h0) / (h1 - h0)
    return table[-1][1]


def corner_stair(name, a, b, kind, half):
    """Stairs turning a corner whose two raised sides face `a` and `b`."""
    return stairs(name, a, half, f"{kind}_{'left' if b == LEFT[a] else 'right'}")


def revolve(g, cx, cz, rows, full, stair, slab_name=None, under=True, corners=True):
    """Solid of revolution, one (layer, radius) per row. Edges stepping in toward the
    axis become stairs, so domes read smooth rather than terraced. Small domes skip the
    corner stairs, which only make them lumpy."""
    layers = {y: set(disc_cells(cx, cz, r)) for y, r in rows if r > 0.25}
    for y, cells in layers.items():
        for x, z in cells:
            g.set(x, y, z, full)
    passes = [(1, "bottom")] + ([(-1, "top")] if under else [])
    for dy, half in passes:
        for y, cells in layers.items():
            other = layers.get(y + dy)
            if not other:
                continue
            for x, z in cells - other:
                dx, dz = x - cx, z - cz
                sx = (dx < -0.01) - (dx > 0.01)
                sz = (dz < -0.01) - (dz > 0.01)
                a = sx != 0 and (x + sx, z) in other
                b = sz != 0 and (x, z + sz) in other
                d = sx != 0 and sz != 0 and (x + sx, z + sz) in other
                fx = OUT.get((sx, 0))
                fz = OUT.get((0, sz))
                if not corners:
                    if half == "bottom" and slab_name and (a or b or d):
                        g.set(x, y, z, slab_name)
                    continue
                if a and b:
                    name = corner_stair(stair, fz, fx, "inner", half)
                elif a:
                    name = stairs(stair, fx, half)
                elif b:
                    name = stairs(stair, fz, half)
                elif d and corners:
                    name = corner_stair(stair, fz, fx, "outer", half)
                elif slab_name and half == "bottom":
                    name = slab_name
                else:
                    continue
                g.set(x, y, z, name)
    return layers


NICE = {False: [0.5, 1.45, 2.3, 3.2, 4.25, 5.1], True: [0.75, 1.6, 2.6, 3.6, 4.6]}


def post_cells(cells, cx, cz):
    """Columns of a kiosk on floor `cells`: one block at each corner of the octagon, open
    between; a ring too small for eight openings gets its four corners."""
    ring = [c for c in cells if any((c[0] + dx, c[1] + dz) not in cells for dx, dz in OUT)]
    if len(ring) <= 12:
        m = max(min(abs(x - cx), abs(z - cz)) for x, z in ring)
        return {(x, z) for x, z in ring if min(abs(x - cx), abs(z - cz)) >= m - 0.01}
    posts = set()
    for k in range(8):
        a = math.radians(22.5 + 45 * k)

        def miss(c):
            d = math.atan2(c[1] - cz, c[0] - cx) - a
            return abs(math.atan2(math.sin(d), math.cos(d))), -math.hypot(c[0] - cx, c[1] - cz)

        posts.add(min(ring, key=miss))
    return posts


def nice(r, half):
    """Largest radius not above r (plus a little) whose disc is round, for small domes:
    `half` when the axis falls on a block corner."""
    fits = [n for n in NICE[half] if n <= r + 0.3]
    return fits[-1] if fits else 0.0


def ring_cells(cx, cz, r_out, r_in, shape=disc_cells):
    return set(shape(cx, cz, r_out)) - set(shape(cx, cz, r_in))


def radial_stairs(g, cells, cx, cz, y, stair, half):
    """Stairs on a ring, each raised toward the axis: corbels (top half) or eaves."""
    for x, z in cells:
        dx, dz = x - cx, z - cz
        if abs(dx) >= abs(dz):
            facing = "west" if dx > 0 else "east"
        else:
            facing = "north" if dz > 0 else "south"
        g.set(x, y, z, stairs(stair, facing, half))


def to_model(face, u, v):
    """Face-local (u along the face, v outward from the centre) to model x, z."""
    return {"south": (u, v), "north": (-u, -v), "east": (v, -u), "west": (-v, u)}[face]


FACES = ("south", "north", "east", "west")


# ---------------------------------------------------------------- mausoleum

PLINTH_TOP = 6       # first layer above the 5.8 m marble plinth
PLINTH_HALF = 47     # 95 m square
BODY_HALF = 28       # 57 m square
CHAMFER = 49         # a + b above this is cut: 7 m chamfers
ROOF = 28            # parapet layer; the roof floor is the layer below
PISHTAQ_TOP = 34
MIN_C = 47.5         # the minarets stand on the plinth corners
CHHATRI_C = 16.5

# Outer dome, radius by height above the terrace, traced from the telephoto.
DOME = [(41.0, 13.6), (41.5, 13.8), (42.5, 14.0), (43.5, 14.25), (44.5, 14.45),
        (45.5, 14.55), (46.5, 14.63), (48.0, 14.63), (49.0, 14.55), (50.3, 14.4),
        (50.9, 14.2), (51.5, 14.03), (52.1, 13.77), (52.8, 13.52), (53.4, 13.19),
        (54.0, 12.81), (54.6, 12.39), (55.3, 11.97), (55.9, 11.6), (56.5, 10.9),
        (57.1, 10.31), (57.8, 9.62), (58.4, 8.87), (59.0, 8.09), (59.65, 7.27),
        (60.3, 6.48), (60.9, 6.34), (61.2, 6.24), (61.5, 5.49), (61.8, 4.74),
        (62.15, 4.33), (62.8, 3.5), (63.4, 2.86), (64.0, 2.25), (64.65, 1.67), (65.3, 1.1)]
DRUM_R = 13.4
DRUM_TOP = 40


def iwan_arch(u):
    return 20.5 + pointed(u, 6.5, 6.0)


def build_plinth(g):
    h = PLINTH_HALF
    g.fill(-h, h, 0, PLINTH_TOP - 1, -h, h, MARBLE_B)
    # Marble floor laid in squares.
    for x in range(-h, h + 1):
        for z in range(-h, h + 1):
            g.set(x, PLINTH_TOP - 1, z, MARBLE_B if x % 4 == 0 or z % 4 == 0 else MARBLE)
    # Blind arcade round the plinth: panels over arched niches on a 3 m bay.
    for face in FACES:
        for u in range(-h + 3, h - 2):
            x, z = to_model(face, u, h)
            xi, zi = to_model(face, u, h - 1)
            if u % 3 == 0:
                g.column(x, z, 0, PLINTH_TOP - 2, MARBLE)
                continue
            for y in (1, 2):
                g.set(x, y, z, AIR)
                g.set(xi, y, zi, MARBLE_CARVED)
            g.set(x, 3, z, MARBLE)
            g.set(x, 4, z, AIR)
            g.set(xi, 4, zi, MARBLE_CARVED)
    for face in FACES:
        for u in range(-h - 1, h + 2):
            x, z = to_model(face, u, h + 1)
            g.set(x, PLINTH_TOP - 1, z, marble_stairs(OPP[face], "top"))
            g.set(x, 0, z, marble_stairs(OPP[face]))
        for u in range(-h, h + 1):
            x, z = to_model(face, u, h)
            g.set(x, PLINTH_TOP, z, MARBLE_PILLAR if u % 3 == 0 else MARBLE_SLAB)
    # Octagonal bastions under the minarets, railed round.
    for sx in (-1, 1):
        for sz in (-1, 1):
            cells = set(octagon_cells(sx * MIN_C, sz * MIN_C, 5.0))
            for x, z in cells:
                if abs(x) > h or abs(z) > h:
                    g.column(x, z, 0, PLINTH_TOP - 1, MARBLE_B)
                    g.set(x, PLINTH_TOP, z, AIR)
            for x, z in cells:
                if any((x + dx, z + dz) not in cells for dx, dz in OUT):
                    if abs(x) > h - 1 or abs(z) > h - 1:
                        g.set(x, PLINTH_TOP, z, MARBLE_PILLAR if (x + z) % 3 == 0 else MARBLE_SLAB)
    # The stair to the plinth top, in a porch on the middle of the south face.
    for u in range(-9, 10):
        for v in (h + 1, h + 2, h + 3):
            g.column(u, v, 0, PLINTH_TOP - 1, MARBLE_B)
            g.set(u, 0, v, MARBLE_B)
        g.set(u, PLINTH_TOP, h + 3, MARBLE_PILLAR if u % 3 == 0 else MARBLE_SLAB)
        g.set(u, PLINTH_TOP - 1, h + 4, marble_stairs("north", "top"))
    for u in range(-2, 3):
        for y in range(0, 4):
            top = 2.4 + pointed(u, 2.5, 1.6)
            if y + 0.5 <= top:
                g.set(u, y, h + 3, AIR)
                g.set(u, y, h + 2, DARK)


def body_inside(x, z):
    a, b = abs(x), abs(z)
    return a <= BODY_HALF and b <= BODY_HALF and a + b <= CHAMFER


def build_body(g):
    for x in range(-BODY_HALF, BODY_HALF + 1):
        for z in range(-BODY_HALF, BODY_HALF + 1):
            if not body_inside(x, z):
                continue
            g.column(x, z, PLINTH_TOP, ROOF - 1, MARBLE)
            edge = not all(body_inside(x + dx, z + dz) for dx in (-1, 0, 1) for dz in (-1, 0, 1))
            if edge:
                g.set(x, PLINTH_TOP, z, MARBLE_B)
                g.set(x, PLINTH_TOP + 1, z, MARBLE_CARVED)
                g.set(x, ROOF - 2, z, MARBLE_B)
                g.set(x, ROOF - 1, z, CALLIGRAPHY)
                g.set(x, ROOF, z, MARBLE_CARVED)
    for face in FACES:
        build_face(g, face)
    for sx in (-1, 1):
        for sz in (-1, 1):
            build_chamfer(g, sx, sz)


def window(du, y, lo, hi, transom):
    """Two-light jali window: dark panes either side of a marble mullion."""
    if abs(du) == 1 and lo <= y <= hi and y != transom:
        return JALI
    return MARBLE


def niche(g, face, uc, y0, spring, rise, lo, hi):
    """Half-octagonal arched niche, 5 m wide and 3 m deep, a window at its back."""
    for du in range(-2, 3):
        top = spring + pointed(du, 2.5, rise)
        for y in range(y0, math.ceil(top) + 1):
            if y + 0.5 > top:
                continue
            depth = 3 if abs(du) <= 1 else 2
            for d in range(depth):
                x, z = to_model(face, uc + du, BODY_HALF - d)
                g.set(x, y, z, AIR)
            x, z = to_model(face, uc + du, BODY_HALF - depth)
            g.set(x, y, z, window(du, y, lo, hi, (lo + hi) // 2 + 1) if depth == 3 else MARBLE)


def build_face(g, face):
    v = BODY_HALF
    # Side bays: two tiers of niches, each in a frame with carved spandrels.
    for side in (-1, 1):
        uc = side * 17
        niche(g, face, uc, 7, 11.0, 3.4, 8, 12)
        niche(g, face, uc, 16, 21.4, 3.4, 17, 22)
        for du in range(-3, 4):
            for y in range(7, 27):
                x, z = to_model(face, uc + du, v)
                if g.get(x, y, z) == AIR:
                    continue
                if abs(du) == 3 or y in (15, 16, 25, 26):
                    g.set(x, y, z, MARBLE_B)
                else:
                    g.set(x, y, z, MARBLE_CARVED)
    for u in (-11, 11, -21, 21):
        x, z = to_model(face, u, v)
        g.column(x, z, PLINTH_TOP, ROOF, MARBLE_PILLAR)
    # The pishtaq frame rises over the roof.
    for u in range(-11, 12):
        for y in range(ROOF - 2, PISHTAQ_TOP + 1):
            for d in range(2):
                x, z = to_model(face, u, v - d)
                g.set(x, y, z, MARBLE)
        x, z = to_model(face, u, v)
        g.set(x, PISHTAQ_TOP, z, MARBLE_B)
        g.set(x, PISHTAQ_TOP - 1, z, CALLIGRAPHY)
        g.set(x, PISHTAQ_TOP - 2, z, MARBLE_B)
        x, z = to_model(face, u, v + 1)
        g.set(x, PISHTAQ_TOP - 2, z, marble_stairs(OPP[face], "top"))
    for y in range(PLINTH_TOP, PISHTAQ_TOP + 1):
        for u in (-11, 11):
            x, z = to_model(face, u, v)
            g.set(x, y, z, MARBLE_PILLAR)
    # Calligraphy frame round the arch.
    for u in range(-9, 10):
        for y in range(PLINTH_TOP + 2, 31):
            if (abs(u) == 9 and y <= 30) or y in (29, 30):
                x, z = to_model(face, u, v)
                g.set(x, y, z, CALLIGRAPHY)
    # Spandrels over the iwan arch, a black inlay line along the arch.
    for u in range(-8, 9):
        for y in range(16, 29):
            x, z = to_model(face, u, v)
            top = iwan_arch(u)
            if abs(u) == 8 or y == 28:
                g.set(x, y, z, MARBLE_B)
            elif y + 0.5 > top + 1.0:
                g.set(x, y, z, MARBLE_CARVED)
            elif y + 0.5 > top:
                g.set(x, y, z, MARBLE_B)
    # The iwan: a half-octagonal recess under the pointed arch.
    for u in range(-6, 7):
        top = iwan_arch(u)
        for dv in range(0, 6):
            if abs(u) > min(6, 8 - dv):
                continue
            for y in range(PLINTH_TOP, math.ceil(top) + 1):
                if y + 0.5 <= top - dv * 0.25:
                    x, z = to_model(face, u, v - dv)
                    g.set(x, y, z, AIR)
    # Back wall: door below, a big jali window above, a cornice between.
    for u in range(-3, 4):
        x, z = to_model(face, u, v - 6)
        for y in range(PLINTH_TOP, 26):
            door = abs(u) <= 1 and y + 0.5 <= 10 + pointed(u, 1.5, 1.5)
            win = abs(u) <= 2 and 15 <= y and y + 0.5 <= 19 + pointed(u, 2.5, 2.0)
            if door:
                g.set(x, y, z, DARK)
            elif win:
                g.set(x, y, z, MARBLE if u == 0 or y == 18 else JALI)
            elif y == 13:
                g.set(x, y, z, MARBLE_B)
    # Angled iwan sides carry small niches.
    for side in (-1, 1):
        for y in range(8, 12):
            x, z = to_model(face, side * 5, v - 4)
            g.set(x, y, z, DARK if y < 11 else MARBLE)
        for y in range(15, 19):
            x, z = to_model(face, side * 5, v - 4)
            g.set(x, y, z, DARK if y < 18 else MARBLE)


def build_chamfer(g, sx, sz):
    """Two tiers of niches on the 45 degree chamfer of quadrant (sx, sz)."""
    for x in range(0, BODY_HALF + 1):
        for z in range(0, BODY_HALF + 1):
            t = x + z
            s = (x - z) / math.sqrt(2)
            if t > CHAMFER or t < CHAMFER - 3 or abs(s) > 2.85:
                continue
            for y in range(7, 26):
                low = y + 0.5 <= 11.0 + pointed(s, 2.85, 3.4)
                up = 16 <= y and y + 0.5 <= 21.4 + pointed(s, 2.85, 3.4)
                if not (low or up):
                    continue
                if t > CHAMFER - 3:
                    g.set(sx * x, y, sz * z, AIR)
                else:
                    lo, hi = (8, 12) if low else (17, 22)
                    pane = abs(s) < 1.2 and lo <= y <= hi and y != (lo + hi) // 2 + 1
                    g.set(sx * x, y, sz * z, JALI if pane else MARBLE)
    for (x, z) in ((21, 28), (28, 21)):
        g.column(sx * x, sz * z, PLINTH_TOP, ROOF, MARBLE_PILLAR)
    # Frames round the chamfer niches.
    for x in range(0, BODY_HALF + 1):
        for z in range(0, BODY_HALF + 1):
            if x + z != CHAMFER:
                continue
            s = abs(x - z) / math.sqrt(2)
            for y in range(7, 27):
                here = g.get(sx * x, y, sz * z)
                if here in (AIR, MARBLE_PILLAR):
                    continue
                frame = s > 3.5 or y in (15, 16, 25, 26)
                g.set(sx * x, y, sz * z, MARBLE_B if frame else MARBLE_CARVED)


def guldasta(g, x, z, y0, top):
    """Slender pinnacle: a fluted shaft, a carved cap, a gilt tip."""
    g.column(x, z, y0, top - 2, MARBLE_PILLAR)
    g.set(x, top - 1, z, MARBLE_CARVED)
    g.set(x, top, z, ROD)


def build_roof(g):
    for x, z in disc_cells(0, 0, DRUM_R):
        g.column(x, z, ROOF - 1, DRUM_TOP, MARBLE)
    rim = ring_cells(0, 0, DRUM_R, DRUM_R - 1)
    for x, z in rim:
        k = int((math.atan2(z, x) + math.pi) / (2 * math.pi) * 64)
        g.set(x, 35, z, MARBLE_B)
        g.set(x, 36, z, MARBLE_CARVED if k % 2 == 0 else MARBLE)
        g.set(x, 37, z, MARBLE_B)
        g.set(x, 38, z, CALLIGRAPHY)
        g.set(x, 39, z, MARBLE_CARVED if k % 2 == 0 else MARBLE)
        g.set(x, DRUM_TOP, z, MARBLE_B)
    # Onion dome and lotus crown, smoothed with stairs.
    rows = [(y, interp(DOME, y + 0.5)) for y in range(DRUM_TOP + 1, 66)]
    revolve(g, 0, 0, rows, MARBLE, "smooth_quartz_stairs", MARBLE_SLAB)
    # Inlaid band round the foot of the bulb, where its side stands upright.
    for y in (42, 43, 44, 45):
        r = interp(DOME, y + 0.5)
        for x, z in ring_cells(0, 0, r, r - 1.5):
            if g.get(x, y, z) == MARBLE:
                g.set(x, y, z, CALLIGRAPHY if y in (43, 44) else MARBLE_B)
    # Gilt finial: a stack of bulbs under the crescent, its horns either side of the spike.
    for y in range(66, 73):
        g.set(0, y, 0, GOLD)
    g.set(-1, 73, 0, GOLD)
    g.set(1, 73, 0, GOLD)
    g.set(0, 73, 0, ROD)
    for sx in (-1, 1):
        for sz in (-1, 1):
            chhatri(g, sx * CHHATRI_C, sz * CHHATRI_C)
            roof_kiosk(g, sx, sz)
    for face in FACES:
        for u in (-11, 11):
            x, z = to_model(face, u, BODY_HALF)
            guldasta(g, x, z, PISHTAQ_TOP + 1, 39)
    for sx in (-1, 1):
        for sz in (-1, 1):
            for (x, z) in ((21, 28), (28, 21)):
                guldasta(g, sx * x, sz * z, ROOF + 1, 38)


def roof_kiosk(g, sx, sz):
    """Stair head behind each chamfer: a small pavilion with a dark door."""
    cells = [(sx * x, sz * z) for x in (23, 24) for z in (23, 24)]
    for x, z in cells:
        g.column(x, z, ROOF, ROOF + 3, MARBLE)
        g.set(x, ROOF + 4, z, MARBLE_SLAB)
    g.set(sx * 24, ROOF + 1, sz * 24, DARK)
    g.set(sx * 24, ROOF + 2, sz * 24, DARK)
    for x in (22, 23, 24, 25):
        for z in (22, 23, 24, 25):
            if (sx * x, sz * z) not in cells and x + z <= CHAMFER:
                g.set(sx * x, ROOF + 3, sz * z, MARBLE_SLAB_TOP)


def chhatri(g, cx, cz):
    base = set(octagon_cells(cx, cz, 6.0))
    for x, z in base:
        g.set(x, ROOF, z, MARBLE_B)
        g.set(x, ROOF + 1, z, MARBLE)
    for x, z in base - set(octagon_cells(cx, cz, 5.0)):
        g.set(x, ROOF + 1, z, MARBLE_CARVED)
    floor = set(octagon_cells(cx, cz, 4.6))
    posts = post_cells(floor, cx, cz)
    for x, z in floor - set(octagon_cells(cx, cz, 3.6)):
        column = (x, z) in posts
        for y in range(ROOF + 2, ROOF + 6):
            g.set(x, y, z, MARBLE_PILLAR if column else AIR)
        if not column:
            g.set(x, ROOF + 5, z, MARBLE_CARVED)
    for x, z in octagon_cells(cx, cz, 4.6):
        g.set(x, ROOF + 6, z, MARBLE_B)
    radial_stairs(g, set(octagon_cells(cx, cz, 5.6)) - set(octagon_cells(cx, cz, 4.6)),
                  cx, cz, ROOF + 6, "smooth_quartz_stairs", "top")
    for x, z in disc_cells(cx, cz, 3.9):
        g.set(x, ROOF + 7, z, MARBLE_B)
    rows = [(36, 4.2), (37, 4.45), (38, 4.4), (39, 3.95), (40, 3.1), (41, 1.9)]
    revolve(g, cx, cz, rows, MARBLE, "smooth_quartz_stairs", MARBLE_SLAB)
    x, z = math.floor(cx), math.floor(cz)
    g.set(x, 42, z, GOLD)
    g.set(x, 43, z, GOLD)
    g.set(x, 44, z, ROD)


def minaret(g, cx, cz):
    def disc(y, r, name):
        for x, z in disc_cells(cx, cz, r):
            g.set(x, y, z, name)

    for y in (6, 7):
        for x, z in octagon_cells(cx, cz, 3.6):
            g.set(x, y, z, MARBLE_B if y == 6 else MARBLE_CARVED)
    for y0, y1, r in ((8, 16, 2.95), (17, 27, 2.6), (28, 39, 2.2)):
        for y in range(y0, y1 + 1):
            disc(y, r, MARBLE_BRICK)
    for floor, r_out, r_shaft in ((16, 3.5, 2.95), (27, 3.3, 2.6), (39, 3.0, 2.2)):
        rim = ring_cells(cx, cz, r_out, r_shaft)
        radial_stairs(g, rim, cx, cz, floor - 1, "smooth_quartz_stairs", "top")
        for x, z in rim:
            g.set(x, floor, z, MARBLE_B)
            g.set(x, floor + 1, z, MARBLE_CARVED)
    # Chhatri: an open ring of columns, eave, dome, finial.
    floor = set(disc_cells(cx, cz, 2.2))
    posts = post_cells(floor, cx, cz)
    for y in (40, 41, 42):
        for x, z in floor:
            g.set(x, y, z, MARBLE_PILLAR if (x, z) in posts else AIR)
    for x, z in disc_cells(cx, cz, 2.2):
        g.set(x, 43, z, MARBLE_B)
    radial_stairs(g, ring_cells(cx, cz, 3.0, 2.2), cx, cz, 43, "smooth_quartz_stairs", "top")
    revolve(g, cx, cz, [(44, 2.2), (45, 2.0), (46, 1.2)], MARBLE, "smooth_quartz_stairs",
            MARBLE_SLAB)
    x, z = math.floor(cx), math.floor(cz)
    g.set(x, 47, z, GOLD)
    g.set(x, 48, z, ROD)


def build_interior(g):
    """The octagonal tomb chamber under its inner dome, entered from the four iwans,
    with the two cenotaphs inside the octagonal jali screen and the hanging lamp."""
    chamber = set(octagon_cells(0, 0, 8.8))
    for x, z in chamber:
        for y in range(PLINTH_TOP, 29):
            g.set(x, y, z, AIR)
        g.set(x, PLINTH_TOP - 1, z, MARBLE_B if (x + z) % 2 == 0 else MARBLE)
    for k, y in enumerate(range(29, 38)):
        r = 8.8 * math.sqrt(max(0.0, 1 - ((y - 28.5) / 9.3) ** 2))
        for x, z in octagon_cells(0, 0, r):
            g.set(x, y, z, AIR)
    wall = set(octagon_cells(0, 0, 9.8)) - chamber
    for x, z in wall:
        g.set(x, PLINTH_TOP, z, MARBLE_CARVED)
        g.set(x, PLINTH_TOP + 1, z, MARBLE_CARVED)
        g.set(x, 13, z, MARBLE_B)
        g.set(x, 22, z, CALLIGRAPHY)
        g.set(x, 28, z, MARBLE_B)
    # Arched recesses on all eight sides, doorways on the four axes.
    for x, z in set(octagon_cells(0, 0, 10.8)) - chamber:
        ang = math.atan2(z, x)
        side = (math.degrees(ang) + 22.5) % 45 - 22.5
        off = abs(math.hypot(x, z) * math.sin(math.radians(side)))
        for y in range(PLINTH_TOP + 2, 22):
            low = y + 0.5 <= 10.5 + pointed(off, 2.6, 2.2)
            up = 15 <= y and y + 0.5 <= 18.5 + pointed(off, 2.6, 2.2)
            if off <= 2.4 and (low or up):
                g.set(x, y, z, AIR if math.hypot(x, z) < 10.4 else (DARK if up else MARBLE))
    # Visitors come in from the south; the other doors stay shut behind their screens.
    for face in ("south",):
        for u in (-1, 0, 1):
            for v in range(8, BODY_HALF - 5):
                for y in range(PLINTH_TOP, 11):
                    if y + 0.5 <= 9.0 + pointed(u, 1.5, 1.5):
                        x, z = to_model(face, u, v)
                        g.set(x, y, z, AIR)
    # The jali screen round the cenotaphs, open to the south.
    for x, z in set(octagon_cells(0, 0, 4.5)) - set(octagon_cells(0, 0, 3.5)):
        if x == 0 and z > 0:
            continue
        g.set(x, PLINTH_TOP, z, MARBLE_CARVED)
        g.set(x, PLINTH_TOP + 1, z, MARBLE_SLAB)
    for x in (0, -2):
        for z in (-1, 0, 1):
            g.set(x, PLINTH_TOP, z, MARBLE_CARVED)
            g.set(x, PLINTH_TOP + 1, z, MARBLE_SLAB)
    # The lamp hangs on one long chain from the crown of the inner dome.
    for y in range(17, 38):
        g.set(0, y, 0, state("chain", axis="y", waterlogged=False))
    g.set(0, 16, 0, state("lantern", hanging=True, waterlogged=False))


def build_mausoleum(g):
    build_plinth(g)
    build_body(g)
    build_roof(g)
    build_interior(g)
    for sx in (-1, 1):
        for sz in (-1, 1):
            minaret(g, sx * MIN_C, sz * MIN_C)


# ---------------------------------------------------------------- domes and kiosks

# The main dome's silhouette, normalised: height over the bulb, radius over the widest.
ONION = [((h - 41.0) / 19.9, r / 14.63) for h, r in DOME if h <= 60.9]


def onion(g, cx, cz, neck, rmax, height, body=MARBLE):
    """Onion dome on (cx, cz) from layer `neck`: the main dome's shape scaled, a lotus cap
    and a gilt finial. Returns the layer of the finial tip."""
    if rmax >= 5:
        rows = [(y, rmax * interp(ONION, (y + 0.5 - neck) / height))
                for y in range(neck, neck + math.ceil(height))]
    else:
        # Kiosk domes are near hemispheres; each layer snaps to a round disc.
        height = rmax * 1.15
        half = abs(cx - math.floor(cx) - 0.5) < 0.01
        rows = [(neck + k, nice(rmax * math.sqrt(max(0.0, 1 - ((k + 0.3) / height) ** 2)), half))
                for k in range(math.ceil(height))]
    top = neck + math.ceil(height)
    lotus = round(rmax * 0.25) if rmax >= 5 else 0
    rows += [(top + k, min(rows[-1][1], rmax * 0.4 * (1 - k / max(lotus, 1)) + 0.35))
             for k in range(lotus)]
    big = rmax >= 5
    revolve(g, cx, cz, rows, body, "smooth_quartz_stairs", MARBLE_SLAB, under=big, corners=big)
    x, z = math.floor(cx), math.floor(cz)
    tip = top + lotus
    for k in range(max(1, round(rmax * 0.3))):
        g.set(x, tip, z, GOLD)
        tip += 1
    g.set(x, tip, z, ROD)
    return tip


def kiosk(g, cx, cz, y0, r, posts, pier=SAND_RED):
    """Open pavilion: floor, `posts` high piers on an octagon of inradius r, a stair eave,
    a white dome."""
    cells = set(octagon_cells(cx, cz, r))
    for x, z in cells:
        g.set(x, y0, z, pier)
    columns = post_cells(cells, cx, cz)
    for x, z in cells:
        for y in range(y0 + 1, y0 + 1 + posts):
            g.set(x, y, z, pier if (x, z) in columns else AIR)
    top = y0 + 1 + posts
    for x, z in cells:
        g.set(x, top, z, pier)
    radial_stairs(g, set(octagon_cells(cx, cz, r + 1.0)) - cells, cx, cz, top,
                  "granite_stairs", "top")
    return onion(g, cx, cz, top + 1, max(1.3, r * 0.9), max(2.0, r * 1.3))


# ---------------------------------------------------------------- terrace

TERRACE_X = 150      # outer faces of the boundary walls
TERRACE_N, TERRACE_S = -56, 56
TOWERS = [(sx * 146, sz * 52) for sx in (-1, 1) for sz in (-1, 1)]
POOL_X = 78


def paving(x, z):
    """Red sandstone floor with a paler trellis, as on the chameli farsh."""
    return PAVE_LINE if (x + z) % 10 == 0 or (x - z) % 10 == 0 else SAND_TRIM


def build_terrace(g):
    for x in range(-TERRACE_X, TERRACE_X + 1):
        for z in range(TERRACE_N, TERRACE_S + 1):
            g.set(x, -1, z, paving(x, z))
            g.set(x, -2, z, SAND_RED)
    # River wall: a blind arcade down to the bank, a frieze and a parapet over it.
    for x in range(-TERRACE_X, TERRACE_X + 1):
        for y in range(-9, 0):
            g.set(x, y, TERRACE_N, SAND_RED)
            g.set(x, y, TERRACE_N + 1, SAND_RED)
        k = (x + 3) % 7
        for y in range(-9, -3):
            top = -8.0 + pointed(k - 3, 2.5, 3.0)
            if 1 <= k <= 5 and y + 0.5 <= top:
                g.set(x, y, TERRACE_N, MARBLE if k in (1, 5) or y + 1.5 > top else SAND_TRIM)
        g.set(x, -3, TERRACE_N, MARBLE if x % 2 == 0 else SAND_RED)
        g.set(x, 0, TERRACE_N, SAND_RED)
        g.set(x, 1, TERRACE_N, SAND_SLAB)
    # The east and west sides close against the boundary walls.
    for sx in (-1, 1):
        for z in range(TERRACE_N, TERRACE_S + 1):
            for x in (sx * 149, sx * 150):
                g.column(x, z, -9, 5, SAND_RED)
            for x in (sx * 149, sx * 150):
                g.set(x, 6, z, SAND_RED if z % 2 == 0 else AIR)
            g.set(sx * 148, 4, z, sand_stairs("east" if sx < 0 else "west", "top"))
    for sx in (-1, 1):
        for sz in (-1, 1):
            dalan(g, sx, sz)
    for cx, cz in TOWERS:
        terrace_tower(g, cx, cz)
    for side in (-1, 1):
        mosque(g, side)
        pool(g, side * POOL_X, 0)
    # Steps down from the terrace to the garden walk on the main axis.
    for x in range(-15, 16):
        g.set(x, -1, TERRACE_S, sand_stairs("north"))


def dalan(g, sx, sz):
    """Arcaded gallery against the boundary wall between the mosque and a corner tower."""
    face = sx * 145
    for z in range(sz * 30, sz * 46, sz):
        for x in range(min(face, sx * 150), max(face, sx * 150) + 1):
            g.column(x, z, 0, 5, SAND_RED)
            g.set(x, 6, z, SAND_TRIM if x == face else AIR)
        k = (abs(z) - 30) % 4
        for y in range(0, 5):
            if k != 0 and y + 0.5 <= 2.6 + pointed(k - 2, 1.5, 1.6):
                for d in range(0, 3):
                    g.set(face + sx * d, y, z, AIR)
            elif k == 0 and y == 4:
                g.set(face, y, z, PAVE_LINE)
        g.set(face, 5, z, PAVE_LINE)


def terrace_tower(g, cx, cz, base=None):
    """Octagonal corner tower: a solid storey with niches, an arcaded gallery under a
    stair eave, a railing, then a white-domed kiosk."""
    base = base if base is not None else (-9 if cz < 0 else -2)
    cells = set(octagon_cells(cx, cz, 7.4))
    inner = set(octagon_cells(cx, cz, 6.4))
    for x, z in cells:
        g.column(x, z, base, 6, SAND_RED)

    def face_off(x, z):
        ang = math.degrees(math.atan2(z - cz, x - cx))
        side = (ang + 22.5) % 45 - 22.5
        return abs(math.hypot(x - cx, z - cz) * math.sin(math.radians(side)))

    for x, z in cells - inner:
        off = face_off(x, z)
        for y in range(0, 6):
            if off <= 1.6 and y + 0.5 <= 2.5 + pointed(off, 1.6, 2.0):
                g.set(x, y, z, SAND_TRIM)
            elif off > 2.4 and y in (1, 4):
                g.set(x, y, z, PAVE_LINE)
        g.set(x, 5, z, PAVE_LINE)
    radial_stairs(g, set(octagon_cells(cx, cz, 8.4)) - cells, cx, cz, 6, "granite_stairs", "top")
    for x, z in cells - inner:
        g.set(x, 7, z, SAND_RED if (x + z) % 2 == 0 else AIR)
    for x, z in inner - set(octagon_cells(cx, cz, 5.4)):
        off = face_off(x, z)
        for y in (7, 8, 9):
            arch = (0.8 < off < 2.6 or off < 0.3) and y < 9
            g.set(x, y, z, AIR if arch else SAND_RED)
    for x, z in set(octagon_cells(cx, cz, 5.4)):
        g.column(x, z, 7, 9, DARK)
    for x, z in inner:
        g.set(x, 10, z, SAND_RED)
    radial_stairs(g, cells - inner, cx, cz, 10, "granite_stairs", "top")
    for x, z in inner - set(octagon_cells(cx, cz, 5.4)):
        g.set(x, 11, z, SAND_SLAB)
    kiosk(g, cx, cz, 11, 3.6, 3)


def mosque(g, side):
    """The mosque (west, side -1) and its answer, the jawab (east, side 1)."""
    front = side * 128
    inward = -side           # the facade faces the mausoleum
    face = "east" if side < 0 else "west"

    def at(u, d):
        """u along the facade (south positive), d back from the facade."""
        return front - inward * d, u

    # Platform with steps, the hall walls, a frieze under the parapet.
    for u in range(-31, 32):
        for d in range(-6, 23):
            x, z = at(u, d)
            g.set(x, 0, z, paving(x, z) if abs(u) < 31 and d > -6 else SAND_RED)
        x, z = at(u, -6)
        g.set(x, 0, z, sand_stairs(OPP[face]))
    for u in range(-28, 29):
        for d in range(0, 23):
            x, z = at(u, d)
            g.column(x, z, 1, 21, SAND_RED)
            edge = d in (0, 22) or abs(u) == 28
            if edge:
                g.set(x, 19, z, SAND_TRIM)
                g.set(x, 20, z, MARBLE if u % 2 == 0 else SAND_TRIM)
                g.set(x, 21, z, SAND_TRIM)

    def panel(u0, u1, y0, y1, dd=0):
        """Outline of a sunk panel; the thin white inlay reads paler at a metre."""
        for u in range(u0, u1 + 1):
            for y in range(y0, y1 + 1):
                if u in (u0, u1) or y in (y0, y1):
                    x, z = at(u, dd)
                    g.set(x, y, z, PAVE_LINE)

    for s in (-1, 1):
        # Narrow panels beside the corner turrets, stacked three high.
        a, b = sorted((s * 22, s * 25))
        for y0 in (2, 8, 14):
            panel(a, b, y0, y0 + 5)
        # Bay: arched doorway below, a blind niche with a window above.
        uc = s * 16
        a, b = sorted((s * 12, s * 20))
        panel(a, b, 1, 9)
        panel(a, b, 10, 19)
        # Tall arched opening into the prayer hall, a small window panel above it.
        for du in range(-2, 3):
            for y in range(1, 11):
                if y + 0.5 <= 6.2 + pointed(du, 2.5, 3.0):
                    for d in range(0, 4):
                        x, z = at(uc + du, d)
                        g.set(x, y, z, AIR)
                    x, z = at(uc + du, 4)
                    g.set(x, y, z, DARK)
            for y in range(12, 18):
                if y + 0.5 <= 15.2 + pointed(du, 1.5, 1.4) and abs(du) <= 1:
                    x, z = at(uc + du, 0)
                    g.set(x, y, z, AIR)
                    x, z = at(uc + du, 1)
                    g.set(x, y, z, DARK if du == 0 and y <= 15 else MARBLE)
        for u in (s * 11, s * 21):
            x, z = at(u, 0)
            g.column(x, z, 1, 24, MARBLE_PILLAR)
            g.set(x, 25, z, MARBLE_CARVED)
            g.set(x, 26, z, ROD)
    # The pishtaq: a broad white marble frame round a deep pointed arch.
    for u in range(-10, 11):
        for y in range(1, 22):
            x, z = at(u, 0)
            au = abs(u)
            if au == 10 or y >= 20:
                continue
            if au in (8, 9) or y in (18, 19):
                g.set(x, y, z, MARBLE)
            elif au == 7 or y == 17:
                g.set(x, y, z, SAND_RED)
            elif y + 0.5 > 11.5 + pointed(u, 6.5, 4.4):
                g.set(x, y, z, MARBLE_CARVED)
            else:
                g.set(x, y, z, MARBLE_B)
    for u in range(-5, 6):
        top = 11.5 + pointed(u, 5.5, 3.5)
        for d in range(0, 6):
            if abs(u) > min(5, 8 - d):
                continue
            for y in range(1, 16):
                if y + 0.5 <= top:
                    x, z = at(u, d)
                    g.set(x, y, z, AIR)
        for y in range(1, 16):
            x, z = at(u, 6)
            if y + 0.5 <= top:
                door = abs(u) <= 1 and y + 0.5 <= 5.5 + pointed(u, 1.5, 1.5)
                g.set(x, y, z, DARK if door else (MARBLE if y == 9 else SAND_RED))
    # Side elevations: three bays of niches in two tiers.
    for end in (-1, 1):
        for dc in (5, 11, 17):
            for dd in (-2, -1, 0, 1, 2):
                for tier, (y0, spring) in enumerate(((2, 6.0), (11, 15.0))):
                    for y in range(y0, 19):
                        if y + 0.5 <= spring + pointed(dd, 2.5, 2.0):
                            x, z = at(end * 28, dc + dd)
                            g.set(x, y, z, MARBLE if abs(dd) == 2 else AIR)
                            xi, zi = at(end * 27, dc + dd)
                            win = abs(dd) <= 1 and y >= y0 + 1 and y + 1.5 <= spring + 2
                            g.set(xi, y, zi, DARK if win else SAND_RED)
    # Octagonal corner turrets, a little proud of both faces, each under an open kiosk.
    for u in (-27.5, 27.5):
        for d in (0.5, 21.5):
            x, z = at(u, d)
            for xx, zz in octagon_cells(x, z, 2.0):
                g.column(xx, zz, 1, 23, SAND_RED)
                g.set(xx, 20, zz, MARBLE)
            kiosk(g, x, z, 24, 2.2, 2)
    # Three white domes on drums.
    # Squatter onions than the tomb's: from the photo, the bulb is as tall as it is wide.
    x, z = at(0, 11)
    for xx, zz in disc_cells(x, z, 6.0):
        g.column(xx, zz, 21, 22, SAND_RED)
        g.set(xx, 22, zz, MARBLE_B)
    onion(g, x, z, 23, 7.0, 9.0)
    for s in (-1, 1):
        x, z = at(s * 16, 11)
        for xx, zz in disc_cells(x, z, 4.0):
            g.column(xx, zz, 21, 21, SAND_RED)
            g.set(xx, 22, zz, MARBLE_B)
        onion(g, x, z, 23, 5.0, 6.5)


def pool(g, cx, cz):
    """Raised tank in front of the mosque or jawab."""
    for x in range(cx - 12, cx + 13):
        for z in range(cz - 12, cz + 13):
            edge = abs(x - cx) == 12 or abs(z - cz) == 12
            g.set(x, 0, z, SAND_SLAB if edge else paving(x, z))
    for x in range(cx - 5, cx + 6):
        for z in range(cz - 6, cz + 7):
            rim = abs(x - cx) == 5 or abs(z - cz) == 6
            g.set(x, 0, z, SAND_SLAB if rim else AIR)
            if not rim:
                g.set(x, -1, z, WATER)
                g.set(x, -2, z, MARBLE)


# ---------------------------------------------------------------- charbagh

G_X = 148            # inner faces of the garden walls
G_N, G_S = 57, 353   # Koch's 296.3 m square
G_C = 205            # cross axis and the raised tank
GROUND = -2          # garden turf; its top is one metre under the terrace
KHIYABAN = 15        # half-width of the main walks
SPRUCE = state("spruce_leaves", distance=1, persistent=True, waterlogged=False)
CANOPY = state("oak_leaves", distance=1, persistent=True, waterlogged=False)
OAK = state("oak_log", axis="y")
TRUNK = state("spruce_log", axis="y")
CHANNEL_BED = state("smooth_stone")
NOZZLE = state("lightning_rod", facing="up", powered=False, waterlogged=True)


def bands(lo, hi):
    """Parterre spans on one side of an axis: two beds and the walk between."""
    mid = (lo + hi) // 2
    return [(lo, mid - 4), (mid + 4, hi)], (mid - 3, mid + 3)


def build_garden(g):
    # Turf everywhere, then walks, then channels.
    for x in range(-G_X, G_X + 1):
        for z in range(G_N, G_S + 1):
            g.set(x, GROUND, z, GRASS)
            g.set(x, GROUND - 1, z, state("dirt"))
    walk = SAND_TRIM

    def pave(x0, x1, z0, z1, name=walk):
        x0, x1 = sorted((x0, x1))
        for x in range(x0, x1 + 1):
            for z in range(z0, z1 + 1):
                if -G_X <= x <= G_X and G_N <= z <= G_S:
                    g.set(x, GROUND, z, name)

    # Perimeter walks along the walls and the terrace.
    pave(-G_X, G_X, G_N, G_N + 5)
    pave(-G_X, G_X, G_S - 5, G_S)
    pave(-G_X, -G_X + 5, G_N, G_S)
    pave(G_X - 5, G_X, G_N, G_S)
    # Secondary walks quartering each quadrant.
    for sx in (-1, 1):
        lo, hi = KHIYABAN + 1, G_X - 6
        _, (w0, w1) = bands(lo, hi)
        pave(sx * w0, sx * w1, G_N, G_S)
    for z_lo, z_hi in ((G_N + 6, G_C - KHIYABAN - 1), (G_C + KHIYABAN + 1, G_S - 6)):
        _, (w0, w1) = bands(z_lo, z_hi)
        pave(-G_X, G_X, w0, w1)
    # The two khiyabans: channel, kerbs, walks, a turf strip of cypresses, outer walks.
    for axis in ("ns", "ew"):
        for o in range(-KHIYABAN, KHIYABAN + 1):
            a = abs(o)
            name = None
            if a <= 2:
                name = WATER if axis == "ns" else CHANNEL_BED
            elif a == 3:
                name = MARBLE_SLAB if axis == "ns" else SAND_SLAB
            elif a <= 6 or a >= 13:
                name = walk
            if name is None:
                continue
            if axis == "ns":
                for z in range(G_N, G_S + 1):
                    g.set(o, GROUND, z, name)
            else:
                for x in range(-G_X, G_X + 1):
                    if abs(x) > KHIYABAN:
                        g.set(x, GROUND, G_C + o, name)
    # The north-south channel holds water and a line of jets; the cross channel is dry.
    for z in range(G_N, G_S + 1):
        for x in range(-2, 3):
            g.set(x, GROUND - 1, z, MARBLE)
            g.set(x, GROUND, z, WATER)
        if z % 3 == 0:
            g.set(0, GROUND, z, NOZZLE)
        for x in (-3, 3):
            g.set(x, GROUND, z, MARBLE)
            g.set(x, GROUND + 1, z, MARBLE_SLAB)
    for x in range(-G_X + 6, G_X - 5):
        if abs(x) <= KHIYABAN:
            continue
        for o in range(-2, 3):
            g.set(x, GROUND, G_C + o, CHANNEL_BED)
        for o in (-3, 3):
            g.set(x, GROUND + 1, G_C + o, SAND_SLAB)
    hauz(g)
    # Two staggered rows of cypresses on each turf strip.
    for o in (-11, -8, 8, 11):
        shift = 0 if abs(o) == 8 else 5
        for z in range(G_N + 10 + shift, G_S - 7, 10):
            if abs(z - G_C) > KHIYABAN + 4:
                cypress(g, o, z)
        for x in range(KHIYABAN + 6 + shift, G_X - 7, 10):
            for sx in (-1, 1):
                cypress(g, sx * x, G_C + o)
    # The row of shade trees OpenStreetMap maps along the foot of the terrace.
    for x in range(80, 147, 6):
        for sx in (-1, 1):
            shade_tree(g, sx * x, G_N + 4)


def hauz(g):
    """The raised marble tank at the crossing, on a sandstone platform."""
    for dx in range(-17, 18):
        for dz in range(-17, 18):
            x, z = dx, G_C + dz
            a = max(abs(dx), abs(dz))
            g.set(x, GROUND, z, SAND_TRIM)
            if a <= 17:
                g.set(x, GROUND + 1, z, SAND_SLAB if a == 17 else paving(x, z))
            if a <= 11:
                g.set(x, GROUND + 1, z, MARBLE)
                g.set(x, GROUND + 2, z, MARBLE_SLAB if a == 11 else AIR)
            if a <= 6:
                g.set(x, GROUND + 1, z, WATER)
                g.set(x, GROUND, z, MARBLE)
                g.set(x, GROUND + 2, z, AIR)
    for dx, dz in ((0, 0), (-3, -3), (3, -3), (-3, 3), (3, 3)):
        g.set(dx, GROUND + 1, G_C + dz, NOZZLE)
    for dz in (-12, 12):
        for dx in (-1, 0, 1):
            g.set(dx, GROUND + 1, G_C + dz, MARBLE)


def shade_tree(g, x, z):
    """Broadleaf tree with a round crown, as in the row mapped along the terrace foot."""
    g.column(x, z, GROUND + 1, GROUND + 4, OAK)
    for y, r in ((GROUND + 4, 2.3), (GROUND + 5, 3.2), (GROUND + 6, 3.2), (GROUND + 7, 2.3),
                 (GROUND + 8, 1.0)):
        for xx, zz in disc_cells(x, z, r):
            if g.get(xx, y, zz) == AIR:
                g.set(xx, y, zz, CANOPY)


def cypress(g, x, z):
    """Italian cypress: a narrow rounded column 9 m tall, tapering to a point."""
    g.set(x, GROUND + 1, z, TRUNK)
    for y in range(GROUND + 2, GROUND + 10):
        g.set(x, y, z, SPRUCE)
    for y in range(GROUND + 3, GROUND + 8):
        for dx, dz in OUT:
            g.set(x + dx, y, z + dz, SPRUCE)


def garden_wall(g):
    """Crenellated enclosure: west, east and south walls, towers and the drum houses."""
    for sx in (-1, 1):
        for z in range(TERRACE_S + 1, G_S + 3):
            for x in (sx * 149, sx * 150):
                g.column(x, z, GROUND, 6, SAND_RED)
            g.set(sx * 150, 7, z, SAND_RED if z % 2 == 0 else AIR)
            g.set(sx * 149, 7, z, SAND_RED if z % 2 == 0 else AIR)
            k = (z - G_N) % 6
            if 1 <= k <= 4:
                top = 2.5 + pointed(k - 2.5, 2.0, 2.0)
                for y in range(GROUND + 1, 6):
                    if y + 0.5 <= top:
                        g.set(sx * 148, y, z, SAND_RED)
        naubat_khana(g, sx)
    for x in range(-150, 151):
        for z in (G_S + 1, G_S + 2):
            g.column(x, z, GROUND, 6, SAND_RED)
            g.set(x, 7, z, SAND_RED if x % 2 == 0 else AIR)
    for sx in (-1, 1):
        terrace_tower(g, sx * 146, G_S + 3, base=GROUND)


def naubat_khana(g, sx):
    """Drum house on the cross axis: two storeys on a plinth, jali arches below, an open
    loggia above, stair eaves, and a white-domed kiosk on the roof."""
    face, back = sx * 137, sx * 150
    span = range(G_C - 12, G_C + 13)
    for x in range(min(face, back), max(face, back) + 1):
        for z in span:
            g.column(x, z, GROUND, 11, SAND_RED)
            g.set(x, 12, z, SAND_RED)
    for z in span:
        dz = z - G_C
        x = face
        for y in range(-1, 13):
            g.set(x, y, z, SAND_RED)
        g.set(x, 0, z, SAND_TRIM)
        g.set(x - sx, -1, z, sand_stairs("east" if sx < 0 else "west") if abs(dz) <= 5
              else SAND_RED)
        g.set(x - sx, 0, z, SAND_SLAB if abs(dz) > 5 else AIR)
        for y in (6, 11):
            g.set(x - sx, y, z, sand_stairs("east" if sx > 0 else "west", "top"))
        g.set(x, 12, z, SAND_TRIM)
        g.set(x, 13, z, SAND_SLAB)
        if abs(dz) in (6, 12):
            continue
        if abs(dz) > 6:
            # Flanking walls: panels outlined in paler stone, two to a storey.
            for y in range(1, 11):
                if y in (1, 5, 7, 10) or abs(dz) in (7, 9, 11):
                    g.set(x, y, z, PAVE_LINE)
            continue
        c = 4 * round(dz / 4)
        off = dz - c
        if abs(off) > 1:
            continue
        for y in range(1, 6):
            if y + 0.5 <= 3.4 + pointed(off, 1.5, 1.6):
                g.set(x, y, z, AIR)
                g.set(x + sx, y, z, JALI_RED)
        for y in range(7, 11):
            if y + 0.5 <= 8.6 + pointed(off, 1.5, 1.6):
                for d in range(0, 3):
                    g.set(x + sx * d, y, z, AIR)
                g.set(x + sx * 3, y, z, DARK)
        g.set(x, 7, z, SAND_SLAB)
    kiosk(g, sx * 143.5, G_C, 12, 3.6, 4)


# ---------------------------------------------------------------- great gate

GATE_N, GATE_S = 352, 385     # Koch: 41.2 x 34 m; mapped flush with the garden's south edge


def build_gate(g):
    """Darwaza-i-Rauza: red sandstone block with a pishtaq each way, corner turrets,
    a kiosk gallery with eleven small domes over each pishtaq, a hall through it."""
    zc = (GATE_N + GATE_S) / 2
    g.fill(-20, 20, GROUND, 16, GATE_N, GATE_S, SAND_RED)
    for x in range(-20, 21):
        for z in (GATE_N, GATE_S):
            g.set(x, 15, z, SAND_TRIM)
            g.set(x, 16, z, MARBLE if x % 2 == 0 else SAND_TRIM)
    for z in range(GATE_N, GATE_S + 1):
        for x in (-20, 20):
            g.set(x, 15, z, SAND_TRIM)
            g.set(x, 16, z, MARBLE if z % 2 == 0 else SAND_TRIM)
    for face, v0 in (("north", GATE_N), ("south", GATE_S)):
        out = -1 if face == "north" else 1

        def at(u, d, face=face):
            return (u, (GATE_N + d) if face == "north" else (GATE_S - d))

        # Wings: white-outlined panels round two tiers of niches.
        for side in (-1, 1):
            for u in range(10, 17):
                for y in range(GROUND + 1, 16):
                    x, z = at(side * u, 0)
                    line = u in (10, 16) or y in (GROUND + 1, 8, 15)
                    g.set(x, y, z, MARBLE if line else SAND_RED)
            uc = side * 13
            for du in range(-2, 3):
                for tier, (y0, spring, rise) in enumerate(((GROUND + 2, 4.0, 2.4), (9, 11.6, 2.2))):
                    for y in range(y0, 15):
                        if y + 0.5 <= spring + pointed(du, 2.5, rise):
                            x, z = at(uc + du, 0)
                            g.set(x, y, z, AIR)
                            x, z = at(uc + du, 1)
                            win = du == 0 and (10 <= y <= 11 if tier else 1 <= y <= 3)
                            g.set(x, y, z, DARK if win else (MARBLE if abs(du) == 2 else SAND_RED))
        # Pishtaq frame: white calligraphy band, marble spandrels, the arch.
        for u in range(-9, 10):
            for y in range(GROUND + 1, 21):
                x, z = at(u, 0)
                if abs(u) == 9 or y == 20:
                    g.set(x, y, z, SAND_RED)
                elif abs(u) == 8 or y in (18, 19):
                    g.set(x, y, z, CALLIGRAPHY)
                elif abs(u) == 7 or y == 17:
                    g.set(x, y, z, MARBLE)
                elif y + 0.5 > 10.6 + pointed(u, 6.5, 5.4):
                    g.set(x, y, z, MARBLE_CARVED)
        for u in range(-6, 7):
            top = 10.6 + pointed(u, 6.5, 5.4)
            for d in range(0, 7):
                if abs(u) > min(6, 9 - d):
                    continue
                for y in range(GROUND + 1, 17):
                    if y + 0.5 <= top:
                        x, z = at(u, d)
                        g.set(x, y, z, AIR)
            for y in range(GROUND + 1, 17):
                if y + 0.5 > top and y + 0.5 <= top + 1.2:
                    x, z = at(u, 0)
                    g.set(x, y, z, MARBLE)
        for u in range(-3, 4):
            for y in range(GROUND + 1, 12):
                x, z = at(u, 7)
                door = abs(u) <= 2 and y + 0.5 <= 5.5 + pointed(u, 2.5, 2.0)
                g.set(x, y, z, AIR if door else (MARBLE if y == 7 else SAND_RED))
        # Gallery of small arches over the pishtaq, eleven domes on it.
        for u in range(-10, 11):
            for d in range(0, 3):
                x, z = at(u, d)
                for y in range(16, 24):
                    g.set(x, y, z, SAND_RED)
                x, z = at(u, 0)
                g.set(x, 20, z, MARBLE_CARVED)
                g.set(x, 21, z, SAND_RED if u % 2 == 0 else AIR)
                g.set(x, 22, z, SAND_RED if u % 2 == 0 else AIR)
                g.set(x, 23, z, SAND_TRIM)
            if u % 2 == 0:
                x, z = at(u, 1)
                g.set(x, 24, z, MARBLE)
                g.set(x, 25, z, MARBLE_SLAB)
                x, z = at(u, 2)
                g.set(x, 24, z, MARBLE)
        for u in (-10, 10):
            x, z = at(u, 0)
            g.column(x, z, 16, 26, MARBLE_PILLAR)
            g.set(x, 27, z, MARBLE_CARVED)
            g.set(x, 28, z, ROD)
    # The hall through the middle.
    for x in range(-5, 6):
        for z in range(GATE_N + 8, GATE_S - 7):
            for y in range(GROUND + 1, 13):
                hall = abs(x) <= 5 and abs(z - zc) <= 6 and y <= 12
                passage = abs(x) <= 2 and y + 0.5 <= 5.5 + pointed(x, 2.5, 2.0)
                if hall or passage:
                    g.set(x, y, z, AIR)
            g.set(x, GROUND, z, SAND_TRIM)
    # Octagonal corner turrets rising to kiosks.
    for sx in (-1, 1):
        for cz in (GATE_N + 2.5, GATE_S - 2.5):
            cx = sx * 18.5
            for x, z in octagon_cells(cx, cz, 3.1):
                g.column(x, z, GROUND, 17, SAND_RED)
                g.set(x, 16, z, MARBLE)
                g.set(x, 8, z, PAVE_LINE)
            radial_stairs(g, set(octagon_cells(cx, cz, 4.1)) - set(octagon_cells(cx, cz, 3.1)),
                          cx, cz, 17, "granite_stairs", "top")
            kiosk(g, cx, cz, 18, 3.1, 3)


def tufts(g):
    """Sparse short grass on the open lawns, hashed per cell so it stays calm."""
    for x in range(-G_X, G_X + 1):
        for z in range(G_N, G_S + 1):
            if g.get(x, GROUND, z) == GRASS and g.get(x, GROUND + 1, z) == AIR:
                if (x * 73856093 ^ z * 19349663) % 97 < 8:
                    g.set(x, GROUND + 1, z, TUFT)


def main(argv):
    out = argv[1] if len(argv) > 1 else os.path.join(HERE, "taj_mahal.schem")
    g = Grid()
    build_garden(g)
    garden_wall(g)
    build_gate(g)
    build_terrace(g)
    build_mausoleum(g)
    tufts(g)
    data, lo = sponge(g.a, g.names)
    open(out, "wb").write(data)
    used = sorted(set(g.names[i] for i in np.unique(g.a)))
    print(f"{out}: {len(data)} bytes, {int((g.a > 0).sum())} blocks, {len(used)} states")
    print("origin (model 0,0,0) at schematic", (-X0 - lo[0], -Y0 - lo[1], -Z0 - lo[2]))


if __name__ == "__main__":
    main(sys.argv)
