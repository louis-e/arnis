#!/usr/bin/env python3
"""Builds parthenon.schem: the Parthenon on the Acropolis as it stands in the 2020s, at one
block per metre.

    python3 assets/structures/landmarks/build_parthenon.py [OUT.schem]

Plan and ruin state follow the OpenStreetMap 3D parts of way 910010406 (column positions and
stump heights, cella wall courses, pediment remains), checked against the Hellenic Cadastre
orthophoto and 2019-2025 photos; dimensions from the published survey (stylobate 69.5 x 30.9 m,
columns 10.43 m, entablature 3.3 m). The temple turns 13 degrees north of east. It is drawn
square in its own frame (u east along the axis, v south across it; a frame cell spans
1/cos 13 = 1.03 m) and each frame cell is moved onto the model grid by whole-block jogs, one per
bay in the gaps between columns, so every column and entablature block stays identical.
No dependencies beyond the standard library.
"""
import gzip
import io
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DATA_VERSION = 4438

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


def _varint(n):
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        if n:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return out


def sponge(blocks, size):
    w, h, l = size
    palette = {"minecraft:air": 0}
    data = bytearray()
    for y in range(h):
        for z in range(l):
            for x in range(w):
                data += _varint(palette.setdefault(blocks.get((x, y, z), "minecraft:air"),
                                                   len(palette)))
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
    return gzip.compress(out.getvalue(), mtime=0), len(palette)


# ---------------------------------------------------------------- block states


def state(name, **props):
    name = name if ":" in name else "minecraft:" + name
    if not props:
        return name
    inner = ",".join(f"{k}={str(v).lower()}" for k, v in sorted(props.items()))
    return f"{name}[{inner}]"


def slab(name, half="bottom"):
    return state(name, type=half, waterlogged=False)


def wall(name):
    # Connections are filled in once the model grid is known.
    return WALL_MARK + name


WALL_MARK = "@wall:"

# Pentelic marble, weathered honey with grey streaks; restored drums are new white marble.
MARBLE = "smooth_sandstone"
MARBLE_SLAB = "cut_sandstone_slab"
# Drums: banded sandstone, some worn smooth; restored drums are new white marble.
DRUM = "sandstone"
WORN_DRUM = "smooth_sandstone"
NEW_DRUM = "calcite"
NECK = "sandstone_wall"
HONEY = "sandstone"
CAPITAL = "smooth_sandstone"
TRIGLYPH = "smooth_sandstone"
# A wall between triglyphs reads as a sunk panel with its relief standing proud.
METOPE = WALL_MARK + "sandstone_wall"
NEW_MARBLE = "calcite"
CAST = WALL_MARK + "sandstone_wall"
POROS = "sandstone"
STONE_FLOOR = "smooth_sandstone"
# Grey weathering patches in the marble floor.
GREY_PATCH = "calcite"

LIGHT = state("light", level=15, waterlogged=False)

# ---------------------------------------------------------------- frame

# Layers: 0 footing seed, 1 buried course (the ground layer), 2 euthynteria flush with the
# ground, then the steps; layer y spans y-3 .. y-2 m above the ground.
FOOTING, GROUND, EUTHYNTERIA = 0, 1, 2
STEP = 3          # steps 1 and 2 (0 .. 1 m)
STYLOBATE = 4     # top step, a slab: 1.5 m
CAPITAL_Y = 14    # column top 12 m
ARCHITRAVE = (15, 16)
FRIEZE = 17
CORNICE = 18      # slab, top 15.5 m

# Column axes sit on frame grid corners; a 2 m shaft fills the cells either side of them.
FLANK_AXES = [-33, -29, -25, -21, -17, -13, -9, -5, 0, 5, 9, 13, 17, 21, 25, 29, 33]
FRONT_AXES = [-14, -10, -6, -2, 2, 6, 10, 14]
PORCH_U = (-28, 28)
PORCH_V = [-10, -6, -2, 2, 6, 10]

# Whole-block jogs, between cells b-1 and b: one per flank bay, and one per front bay but the
# middle one, so the doorway axis stays straight.
U_JOGS = [(a + b + 1) // 2 if b - a == 4 else (a + b + 1) // 2 for a, b in
          zip(FLANK_AXES, FLANK_AXES[1:])]
V_JOGS = [-12, -8, -4, 4, 8, 12]


def z_shift(u):
    return -sum(1 for b in U_JOGS if b <= u) + sum(1 for b in U_JOGS if b <= 0)


def x_shift(v):
    return sum(1 for b in V_JOGS if b <= v) - sum(1 for b in V_JOGS if b <= 0)


class Frame:
    def __init__(self):
        self.cells = {}

    def put(self, u, y, v, s):
        if s is None:
            self.cells.pop((u, y, v), None)
        else:
            self.cells[(u, y, v)] = s

    def box(self, u0, u1, y0, y1, v0, v1, s):
        for u in range(u0, u1):
            for v in range(v0, v1):
                for y in range(y0, y1):
                    self.put(u, y, v, s)

    def get(self, u, y, v):
        return self.cells.get((u, y, v))


def mid_points(axes):
    """Axes and the bay middles a triglyph sits over, rounded towards the centre."""
    pts = set(axes)
    for a, b in zip(axes, axes[1:]):
        m = (a + b) / 2
        pts.add(int(m) if m == int(m) else (int(m + 0.5) if m < 0 else int(m)))
    return pts


def triglyph_at(c, points):
    """Frieze cell c holds a triglyph: the cell outside each point, so both halves mirror."""
    return (c + 1) in points if c < 0 else c in points


# ---------------------------------------------------------------- the temple


def platform(f):
    # Steps: stylobate [-34, 34) x [-15, 15), each step one cell out and half a metre down.
    f.box(-36, 36, FOOTING, GROUND + 1, -17, 17, POROS)
    f.box(-36, 36, EUTHYNTERIA, EUTHYNTERIA + 1, -17, 17, MARBLE)
    # Three half-metre steps, the stylobate the top one.
    for u in range(-36, 36):
        for v in range(-17, 17):
            ring = max(-u - 1 - 33, u - 33, -v - 1 - 14, v - 14)
            f.put(u, STEP, v, slab(MARBLE_SLAB) if ring >= 2 else MARBLE)
            if ring <= 0:
                f.put(u, STYLOBATE, v, slab(MARBLE_SLAB))
    # Raised floor of the cella and its two porches, one slab step up.
    for u in range(-30, 30):
        for v in range(-11, 11):
            f.put(u, STYLOBATE, v, GREY_PATCH if hashed(u, v) < 0.18 else STONE_FLOOR)


def column(f, u, v, top, white=(), base=STYLOBATE):
    """A Doric column on axis (u, v) up to layer `top` (CAPITAL_Y when whole): a 2 m shaft of
    drums, a 1.5 m necking of walls and the 2 m capital flaring out over it."""
    first = STYLOBATE if base == STYLOBATE else STYLOBATE + 1
    for y in range(first, min(top, CAPITAL_Y - 2) + 1):
        drum = NEW_DRUM if y in white else WORN_DRUM if hashed(u, y, v) < 0.3 else DRUM
        for cu, cv in [(u - 1, v - 1), (u, v - 1), (u - 1, v), (u, v)]:
            f.put(cu, y, cv, drum)
    for cu, cv in [(u - 1, v - 1), (u, v - 1), (u - 1, v), (u, v)]:
        if top >= CAPITAL_Y - 1:
            f.put(cu, CAPITAL_Y - 1, cv, wall(NECK))
        if top >= CAPITAL_Y:
            f.put(cu, CAPITAL_Y, cv, CAPITAL)


def hashed(*k):
    """A stable 0..1 value per cell, for calm mosaics."""
    h = 2166136261
    for v in k:
        h = ((h ^ (v & 0xFFFFFFFF)) * 16777619) & 0xFFFFFFFF
    return (h >> 8) / float(1 << 24)


def stump(f, u, v, height_m, base_m=1.5, base=STYLOBATE):
    """A column broken off `height_m` above its base."""
    column(f, u, v, round(base_m + height_m) + 2, base=base)


# Stumps on the south flank (u axis -> metres standing above the stylobate), from OSM.
SOUTH_STUMPS = {0: 8.7, 5: 3.7, 9: 2.7, 13: 7.7}
# Pronaos columns on the north side, re-erected in part (v axis -> metres above the floor).
PRONAOS_STUMPS = {-10: 3.5, -6: 5.5, -2: 7.5}
# Restored drums of new marble, as seen in 2021-2024 photos of the north colonnade.
NORTH_WHITE = {-25: (6, 9), -21: (5, 10), -17: (8, 11), -13: (5, 7, 12), -9: (9, 10),
               -5: (6,), 0: (7, 11), 5: (5, 8), 9: (10,), 13: (6, 9)}


def colonnade(f):
    for u in FLANK_AXES:
        for v in (-14, 14):
            if v == 14 and u in SOUTH_STUMPS:
                stump(f, u, v, SOUTH_STUMPS[u])
            else:
                column(f, u, v, CAPITAL_Y, white=NORTH_WHITE.get(u, ()) if v < 0 else ())
    for v in FRONT_AXES[1:-1]:
        for u in (-33, 33):
            column(f, u, v, CAPITAL_Y)
    # Porches: six columns each on the cella floor (2 m), as tall as the peristyle tops.
    for v in PORCH_V:
        column(f, PORCH_U[0], v, CAPITAL_Y, base=None)
        if v in PRONAOS_STUMPS:
            stump(f, PORCH_U[1], v, PRONAOS_STUMPS[v], base_m=2.0, base=None)
        else:
            column(f, PORCH_U[1], v, CAPITAL_Y, base=None)


def south_gap(u):
    """Flank cells over the fallen middle of the south colonnade (no entablature)."""
    return -4 <= u <= 15


def entablature(f):
    flank_pts = mid_points(FLANK_AXES)
    front_pts = mid_points(FRONT_AXES)

    # Architrave: two cells deep over every column row, across the corners.
    for u in range(-34, 34):
        for v in (-15, -14, 13, 14):
            if v > 0 and south_gap(u):
                continue
            f.box(u, u + 1, ARCHITRAVE[0], ARCHITRAVE[1] + 1, v, v + 1, MARBLE)
    for v in range(-15, 15):
        for u in (-34, -33, 32, 33):
            f.box(u, u + 1, ARCHITRAVE[0], ARCHITRAVE[1] + 1, v, v + 1, MARBLE)

    # Frieze: triglyph or metope on the outer cell, a backer behind it.
    for side in ("north", "south", "west", "east"):
        if side in ("north", "south"):
            vo, vi = (-15, -14) if side == "north" else (14, 13)
            for u in range(-34, 34):
                if side == "south" and south_gap(u):
                    continue
                tri = triglyph_at(u, flank_pts) or u in (-34, 33)
                if tri:
                    f.put(u, FRIEZE, vo, TRIGLYPH)
                    f.put(u, FRIEZE, vi, MARBLE)
                    continue
                m = metope_state(side, u)
                if m is not None:
                    f.put(u, FRIEZE, vo, m)
                    f.put(u, FRIEZE, vi, MARBLE if m == METOPE else m)
        else:
            uo, ui = (-34, -33) if side == "west" else (33, 32)
            for v in range(-15, 15):
                tri = triglyph_at(v, front_pts) or v in (-15, 14)
                f.put(uo, FRIEZE, v, TRIGLYPH if tri else METOPE)
                f.put(ui, FRIEZE, v, MARBLE)

    # Cornice: a slab over the frieze, one cell proud of it, where it survives.
    for side in ("north", "south", "west", "east"):
        if side in ("west", "east"):
            uo, ui = (-35, -33) if side == "west" else (34, 32)
            for v in range(-16, 16):
                for u in range(min(uo, ui), max(uo, ui) + 1):
                    f.put(u, CORNICE, v, slab(MARBLE_SLAB))
            continue
        vo, vi = (-16, -14) if side == "north" else (15, 13)
        for u in range(-35, 35):
            if not cornice_kept(side, u, flank_pts):
                continue
            for v in range(min(vo, vi), max(vo, vi) + 1):
                f.put(u, CORNICE, v, slab(MARBLE_SLAB))


def metope_state(side, u):
    """North: metopes kept in the western bays, gone further east. South: broken low slabs."""
    if side == "north":
        return METOPE if u < -9 else None
    return slab(MARBLE_SLAB)


def cornice_kept(side, u, pts):
    if u <= -31 or u >= 31:
        return True
    if side == "north":
        if u < -21:
            return True
        # Further east single geison blocks still ride on the triglyphs.
        return triglyph_at(u, pts) and u < 25
    return False


def cella(f):
    floor = STYLOBATE  # cella floor layer, top at 2 m
    # West cross wall with the great door, 10 m tall, and its lintel to the frieze top.
    for u in (-24, -23):
        for v in range(-11, 11):
            for y in range(floor + 1, FRIEZE + 1):
                if -2 <= v <= 1 and y <= CAPITAL_Y:
                    continue
                f.put(u, y, v, course(y))
    # Antae ending the side walls on the porch line.
    for v in (-11, -10, 9, 10):
        for y in range(floor + 1, CAPITAL_Y + 1):
            f.put(-25, y, v, course(y))
    # Side walls of the west room, broken off in steps (east end of each course, frame u).
    south_ends = [-3, -6, -7, -8, -13, -14, -19, -20, -20, -20]
    north_ends = [25, -13, -13, -14, -14, -15, -16, -18, -19, -19]
    for k in range(10):
        y = floor + 1 + k
        for u in range(-22, south_ends[k] + 1):
            f.put(u, y, 9, course(y))
        for u in range(-22, north_ends[k] + 1):
            f.put(u, y, -10, course(y))
    # Lowest course left of the naos walls and the east cross wall, its door gap open.
    for u in range(15, 26):
        f.put(u, floor + 1, 9, course(floor + 1))
    for v in list(range(-10, -2)) + list(range(2, 10)):
        f.put(24, floor + 1, v, course(floor + 1))
    # Architrave and the west frieze over the opisthodomos porch, between the antae.
    for v in range(-11, 11):
        for u in (-29, -28, -27, -26, -25):
            for y in ARCHITRAVE:
                if u >= -27 and u <= -25 and v not in (-11, -10, 9, 10):
                    continue
                f.put(u, y, v, MARBLE)
        f.put(-29, FRIEZE, v, CAST)
        f.put(-28, FRIEZE, v, MARBLE)


def course(y):
    """Isodomic courses: honey and paler marble alternate a metre apart."""
    return MARBLE if y % 2 else HONEY


# Pediment remains: raking corners (cells in from each end) and loose tympanum blocks
# (cells, metres above the cornice), from 2021-2025 photos and the OSM corner parts.
PEDIMENTS = {
    "west": {"corners": (5, 5), "blocks": [(range(-8, -5), 2.0), (range(-3, 1), 1.0),
                                           (range(6, 8), 1.0)]},
    "east": {"corners": (6, 7), "blocks": []},
}
# Copies of the corner figures still on the pediment floors.
PEDIMENT_FIGURES = {"west": [-13, -12], "east": [-13, -12, 11, 12]}


def pediments(f):
    def rise(c):
        return 0.28 * (min(c + 15, 14 - c) + 0.5)

    def pile(u, v, top_m):
        whole = int(top_m)
        for y in range(CORNICE, whole + 3):
            f.put(u, y, v, MARBLE)
        if top_m - whole >= 0.5:
            f.put(u, whole + 3, v, slab(MARBLE_SLAB))

    for side, spec in PEDIMENTS.items():
        uo, ui, out = (-34, -33, -35) if side == "west" else (33, 32, 34)
        n, s = spec["corners"]
        for c in list(range(-15, -15 + n)) + list(range(15 - s, 15)):
            top = 15.5 + rise(c)
            for u in (uo, ui):
                pile(u, c, top)
            # Raking geison, one cell proud along the slope.
            whole = int(top)
            if top - whole >= 0.5:
                f.put(out, whole + 2, c, MARBLE)
                f.put(out, whole + 3, c, slab(MARBLE_SLAB))
            else:
                f.put(out, whole + 2, c, slab(MARBLE_SLAB, "top"))
        for cells, height in spec["blocks"]:
            for c in cells:
                pile(uo, c, 15.5 + height)
                pile(ui, c, 15.5 + height)
        for c in PEDIMENT_FIGURES[side]:
            f.put(uo, CORNICE + 1, c, NEW_MARBLE)


def floodlights(f):
    """Invisible lights in every other intercolumniation, low and under the architrave, and in
    the cella: the ruin is floodlit at night from the rock around it."""
    def gaps(axes):
        return [(a + b) // 2 for a, b in zip(axes, axes[1:])][::2]

    for y in (6, 12):
        for u in gaps(FLANK_AXES):
            for v in (-15, 14):
                f.put(u, y, v, LIGHT)
        for v in gaps(FRONT_AXES):
            for u in (-34, 33):
                f.put(u, y, v, LIGHT)
    for u in (-26, -18, -6, 6, 18, 26):
        f.put(u, 9, 0, LIGHT)


def build():
    f = Frame()
    platform(f)
    colonnade(f)
    entablature(f)
    cella(f)
    pediments(f)
    floodlights(f)
    return f


# ---------------------------------------------------------------- frame -> model grid

ORIGIN_X, ORIGIN_Z = 41, 27


def to_model(f):
    blocks = {}
    for (u, y, v), s in f.cells.items():
        x = u + x_shift(v) + ORIGIN_X
        z = v + z_shift(u) + ORIGIN_Z
        blocks[(x, y, z)] = s
    fill_seams(blocks)
    connect_walls(blocks)
    return blocks


def fill_seams(blocks):
    """Jogs crossing leave single cells unmapped; close those inside solid courses."""
    xs = [k[0] for k in blocks]
    zs = [k[2] for k in blocks]
    ys = sorted({k[1] for k in blocks})
    add = {}
    for y in ys:
        for x in range(min(xs), max(xs) + 1):
            for z in range(min(zs), max(zs) + 1):
                if (x, y, z) in blocks:
                    continue
                n = [blocks.get((x + dx, y, z + dz)) for dx, dz in
                     ((1, 0), (-1, 0), (0, 1), (0, -1))]
                if n[0] and n[1] and n[2] and n[3] and n[0] == n[1]:
                    add[(x, y, z)] = n[0]
                elif n[0] and n[1] and n[2] and n[3] and n[2] == n[3]:
                    add[(x, y, z)] = n[2]
    blocks.update(add)


def full_block(s):
    if s is None or s.startswith(WALL_MARK):
        return False
    return "[" not in s or "type=double" in s


def connect_walls(blocks):
    marked = dict(blocks)
    for k, s in marked.items():
        if not s.startswith(WALL_MARK):
            continue
        x, y, z = k
        above = marked.get((x, y + 1, z))
        sides = {}
        for d, (dx, dz) in (("north", (0, -1)), ("south", (0, 1)), ("west", (-1, 0)),
                            ("east", (1, 0))):
            n = marked.get((x + dx, y, z + dz))
            if n is not None and (n.startswith(WALL_MARK) or full_block(n)):
                sides[d] = "tall" if above is not None else "low"
            else:
                sides[d] = "none"
        blocks[k] = state(s[len(WALL_MARK):], up=True, waterlogged=False, **sides)


def main(argv):
    out = argv[0] if argv else os.path.join(HERE, "parthenon.schem")
    blocks = {k: v if ":" in v else "minecraft:" + v for k, v in to_model(build()).items()}
    size = tuple(max(k[i] for k in blocks) + 1 for i in range(3))
    assert min(k[0] for k in blocks) >= 0 and min(k[2] for k in blocks) >= 0
    data, n = sponge(blocks, size)
    assert n <= 256, n
    with open(out, "wb") as fh:
        fh.write(data)
    print(f"{out}: {size[0]}x{size[1]}x{size[2]}, {len(blocks)} blocks, {n} states")


if __name__ == "__main__":
    main(sys.argv[1:])
