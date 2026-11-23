#!/usr/bin/env python3
"""Builds arc_de_triomphe.schem: the Arc de Triomphe de l'Étoile at one block per metre.

    python3 assets/structures/landmarks/build_arc_de_triomphe.py [OUT.schem]

Footprint, turn and heights come from IGN LiDAR HD (2025 delivery, tiles 0648_6864 and
0648_6865): body 44.8 x 22.3 m, long axis 25.6 degrees east of north, cornice top 38.5 m,
attic cornice 45 m, terrace railing 47.3 m and the skylight dome 49.6 m above the paving under
the vault. Elevation bands, arch sizes (14.6 x 29.2 m and 8.4 x 18.7 m), the attic's shields
and the sculpture layout follow the 1838 survey in the Allgemeine Bauzeitung (plates 180 and
182) and 2010-2026 photographs. The arch is drawn square in its own frame (a across the
depth toward the Champs-Élysées, b along the facades toward the south-south-west); each model
column then takes the frame column under its centre, so the turn keeps arches round and every
course at least a block thick.
No dependencies beyond the standard library.
"""
import gzip
import io
import math
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


def stairs(name, facing, half="bottom"):
    return state(name, facing=facing, half=half, shape="straight", waterlogged=False)


WALL_MARK = "@wall:"


def wall(name):
    return WALL_MARK + name


# Light Lutetian limestone: ashlar courses, smooth mouldings, reliefs a shade greyer.
ASHLAR = "quartz_bricks"
SMOOTH = "smooth_quartz"
SMOOTH_STAIRS = "smooth_quartz_stairs"
SMOOTH_SLAB = "smooth_quartz_slab"
MEANDER = "chiseled_quartz_block"
ORNAMENT = "quartz_pillar"
RELIEF = wall("diorite_wall")
GROUND = "calcite"
SCULPTURE = "calcite"
ANTEFIX = "chiseled_quartz_block"
TOMB = "polished_deepslate"
PAVING = "polished_andesite"
FOOTING = "stone"
BRONZE = "waxed_exposed_cut_copper"
GLASS = "white_stained_glass"
RAILING = "@bars:iron_bars"
POST = wall("polished_blackstone_wall")

# ---------------------------------------------------------------- frame

# Frame cells: a (depth) -11..10 toward the Champs-Élysées, b (width) -22..22 toward the
# south-south-west, y layers. Layer FLOOR is the paving; layer FLOOR + 1 + h spans h..h+1 m.
A0, A1 = -11, 10
B0, B1 = -22, 22
FLOOR = 1


def L(h):
    return FLOOR + 1 + h


TURN = math.radians(25.6)
MAIN_HALF = 7          # main arch cells |b| <= 7: 15 m wide
MAIN_SPRING = 21.7     # 29.2 m crown with a 7.5 m radius
SIDE_HALF = 3.5        # transverse arch cells -4..3: 8 m wide
SIDE_SPRING = 14.68    # 18.68 m crown with a 4 m radius


class Frame:
    def __init__(self):
        self.cells = {}
        # Single blocks put on the model grid after the turn, at the cell under (a, b).
        self.points = []
        # Lights, put into the nearest air cell of a model column near (a, b).
        self.lights = []

    def put(self, a, y, b, s):
        if s is None:
            self.cells.pop((a, y, b), None)
        else:
            self.cells[(a, y, b)] = s

    def get(self, a, y, b):
        return self.cells.get((a, y, b))


def main_open(b, h):
    hc = h + 0.5
    return abs(b) <= MAIN_HALF and (hc < MAIN_SPRING or b * b + (hc - MAIN_SPRING) ** 2 < 7.5 ** 2)


def side_open(a, h):
    ac, hc = a + 0.5, h + 0.5
    return abs(ac) <= SIDE_HALF and (hc < SIDE_SPRING or ac * ac + (hc - SIDE_SPRING) ** 2 < 16)


def body_open(a, h, b):
    if main_open(b, h):
        return True
    return abs(b) > MAIN_HALF and side_open(a, h)


# Faces: (name, outward (da, db), plane cell, horizontal cells along the face).
FACES = {
    "east": ((1, 0), A1, list(range(B0, B1 + 1))),
    "west": ((-1, 0), A0, list(range(B0, B1 + 1))),
    "north": ((0, -1), B0, list(range(A0, A1 + 1))),
    "south": ((0, 1), B1, list(range(A0, A1 + 1))),
}
FACING = {(1, 0): "east", (-1, 0): "west", (0, -1): "north", (0, 1): "south"}


def cell(face, s, d):
    """Frame (a, b) of face cell s at depth d (0 the face, +1 one block proud of it)."""
    (da, db), plane, _ = FACES[face]
    if da:
        return plane + da * d, s
    return s, plane + db * d


def face_open(face, s, h):
    a, b = cell(face, s, 0)
    return body_open(a, h, b)


def put_face(f, face, s, d, h, block):
    a, b = cell(face, s, d)
    f.put(a, L(h), b, block)


def out_stairs(face, half):
    """Stairs one block proud of `face`, their full side against it."""
    (da, db), _, _ = FACES[face]
    return stairs(SMOOTH_STAIRS, FACING[(-da, -db)], half)


# ---------------------------------------------------------------- the arch

# Elevation bands, metres above the paving (1838 survey, checked against the laser scan).
BASE, BASE_TOP, DADO = 0, 1, 5
IMPOST = (19, 20, 21)       # meander band round the whole arch, 19.3 to 21.3 m
PANELS = (23, 29)           # relief panels over the impost band
ARCHITRAVE, FRIEZE = 32, (33, 34)
CORNICE = (35, 36, 37, 38)  # bed moulding, modillions, corona, top 38.5 m
ATTIC = (39, 44)            # attic to its cornice; the terrace is the top of layer 44
SHIELD_H = 42


def figure_heights(n, rows, seed):
    """Deterministic skyline of relief figures, `rows` deep at most, a gap now and then."""
    out = []
    x = seed
    for _ in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        r = (x >> 16) % 10
        out.append(0 if r == 0 else rows - (1 if r < 4 else 0) - (1 if r == 9 else 0))
    return out


def relief_field(f, face, s0, s1, h0, h1, seed, framed=True):
    """Figures (walls) standing on a sunk ground, inside a smooth frame if `framed`."""
    inner = (s0 + 1, s1 - 1, h0 + 1, h1 - 1) if framed else (s0, s1, h0, h1)
    if framed:
        for s in range(s0, s1 + 1):
            for h in (h0, h1):
                put_face(f, face, s, 0, h, SMOOTH)
        for h in range(h0, h1 + 1):
            for s in (s0, s1):
                put_face(f, face, s, 0, h, SMOOTH)
    rows = inner[3] - inner[2] + 1
    heights = figure_heights(inner[1] - inner[0] + 1, rows, seed)
    for i, s in enumerate(range(inner[0], inner[1] + 1)):
        for k, h in enumerate(range(inner[2], inner[3] + 1)):
            put_face(f, face, s, -1, h, GROUND)
            put_face(f, face, s, 0, h, RELIEF if k < heights[i] else None)


def body(f):
    for a in range(A0, A1 + 1):
        for b in range(B0, B1 + 1):
            f.put(a, FLOOR - 1, b, FOOTING)
            f.put(a, FLOOR, b, PAVING if body_open(a, 0, b) else ASHLAR)
            for h in range(0, ATTIC[1] + 1):
                if not body_open(a, h, b):
                    f.put(a, L(h), b, ASHLAR)


# Profile of each band: (h, [(depth, block-or-kind), ...]); 'up'/'down' are stairs proud of
# the face with their full side against it.
PROFILE = {
    BASE: [(0, SMOOTH), (1, "up")],
    BASE_TOP: [(0, SMOOTH)],
    DADO: [(0, SMOOTH), (1, "down")],
    IMPOST[0]: [(0, SMOOTH)],
    IMPOST[1]: [(0, MEANDER)],
    IMPOST[2]: [(0, SMOOTH), (1, "down")],
    ARCHITRAVE: [(0, SMOOTH)],
    CORNICE[0]: [(0, SMOOTH), (1, "down")],
    CORNICE[1]: [(0, SMOOTH), (1, SMOOTH), (2, "down")],
    CORNICE[2]: [(0, SMOOTH), (1, SMOOTH), (2, SMOOTH), (3, "down")],
    CORNICE[3]: [(0, "slab"), (1, "slab"), (2, "slab")],
    ATTIC[0]: [(0, SMOOTH), (1, "up")],
    ATTIC[0] + 1: [(0, SMOOTH)],
    ATTIC[1]: [(0, SMOOTH), (1, "down")],
}


def band_block(face, kind, s):
    if kind == "up":
        return out_stairs(face, "bottom")
    if kind == "down":
        return out_stairs(face, "top")
    if kind == "slab":
        return slab(SMOOTH_SLAB)
    return kind


def bands(f):
    for face in FACES:
        for s in FACES[face][2]:
            for h, parts in PROFILE.items():
                if face_open(face, s, h):
                    continue
                for d, kind in parts:
                    put_face(f, face, s, d, h, band_block(face, kind, s))
    # Corners: each projecting course closes round the corner with plain blocks.
    for da, db in ((1, 1), (1, -1), (-1, 1), (-1, -1)):
        a = A1 if da > 0 else A0
        b = B1 if db > 0 else B0
        for h, parts in PROFILE.items():
            for d, kind in parts:
                if d < 1:
                    continue
                corner = slab(SMOOTH_SLAB) if kind == "slab" else (
                    slab(SMOOTH_SLAB, "top") if kind == "down" else
                    slab(SMOOTH_SLAB) if kind == "up" else SMOOTH)
                for i in range(1, d + 1):
                    f.put(a + da * i, L(h), b + db * d, corner)
                    f.put(a + da * d, L(h), b + db * i, corner)


def archivolts(f):
    """Moulded rings round the arches, keystones at the crowns, imposts at the springing."""
    for face in ("east", "west"):
        for s in FACES[face][2]:
            for h in range(IMPOST[2] + 1, 31):
                r = math.hypot(s, h + 0.5 - MAIN_SPRING)
                if 7.5 <= r < 8.6 and not face_open(face, s, h):
                    put_face(f, face, s, 0, h, SMOOTH)
        for h in (28, 29, 30):
            put_face(f, face, 0, 0, h, MEANDER)
        put_face(f, face, 0, 1, 29, out_stairs(face, "top"))
    for face in ("north", "south"):
        for s in FACES[face][2]:
            for h in range(13, IMPOST[0]):
                r = math.hypot(s + 0.5, h + 0.5 - SIDE_SPRING)
                if 4.0 <= r < 5.0 and not face_open(face, s, h):
                    put_face(f, face, s, 0, h, SMOOTH)
            # Pilaster strips flanking the opening, with their impost.
            if s in (-6, -5, 4, 5):
                for h in range(BASE_TOP + 1, 14):
                    put_face(f, face, s, 0, h, SMOOTH)
                put_face(f, face, s, 1, 14, out_stairs(face, "top"))
        for s in (-1, 0):
            put_face(f, face, s, 0, 18, MEANDER)


def reliefs(f):
    # Main faces: two framed battle panels over the pillars, Fames in the arch spandrels.
    for face, seed in (("east", 3), ("west", 7)):
        for s0 in (-20, 11):
            relief_field(f, face, s0, s0 + 9, PANELS[0], PANELS[1], seed + s0)
        # Fames recline along the archivolt: a two-block band of relief round the ring.
        for s in FACES[face][2]:
            for h in range(IMPOST[2] + 1, ARCHITRAVE):
                r = math.hypot(s, h + 0.5 - MAIN_SPRING)
                if 8.6 <= r < 10.4 and abs(s) <= 9 and not face_open(face, s, h):
                    put_face(f, face, s, 0, h, RELIEF)
                    put_face(f, face, s, -1, h, GROUND)
    # Side faces: one long battle panel, figures in the spandrels of the small arch.
    for face, seed in (("north", 11), ("south", 13)):
        relief_field(f, face, -10, 9, PANELS[0], PANELS[1], seed)
        for s in FACES[face][2]:
            for h in range(15, IMPOST[0]):
                r = math.hypot(s + 0.5, h + 0.5 - SIDE_SPRING)
                if 5.0 <= r < 6.4 and abs(s + 0.5) <= 5 and not face_open(face, s, h):
                    put_face(f, face, s, 0, h, RELIEF)
                    put_face(f, face, s, -1, h, GROUND)
    # The great frieze runs round all four sides.
    for face, seed in (("east", 17), ("west", 19), ("north", 23), ("south", 29)):
        s0, s1 = FACES[face][2][0], FACES[face][2][-1]
        relief_field(f, face, s0 + 1, s1 - 1, FRIEZE[0], FRIEZE[1], seed, framed=False)


def attic(f):
    # Eleven shields on each long side, four and a grilled window on each short side, between
    # palm panels; numbers and spacing from the 1838 elevation and the north facade. Narrow
    # pieces reach three blocks into the wall so the turned grid always shows them.
    def carve(face, s, h, block):
        for d in (0, -1, -2):
            put_face(f, face, s, d, h, block)

    def shield(face, cells):
        for s in cells:
            for h in (SHIELD_H - 1, SHIELD_H, SHIELD_H + 1):
                edge = h != SHIELD_H and len(cells) == 3 and s != cells[1]
                if not edge:
                    carve(face, s, h, SMOOTH)
        carve(face, cells[len(cells) // 2], SHIELD_H, MEANDER)

    def panel(face, s):
        for h in (SHIELD_H - 1, SHIELD_H, SHIELD_H + 1):
            carve(face, s, h, ORNAMENT)

    for face in ("east", "west"):
        for s in range(-22, 23, 4):
            panel(face, s)
        for c in range(-20, 21, 4):
            shield(face, [c - 1, c, c + 1])
    for face in ("north", "south"):
        for s in (-10, -6, -2, 1, 5, 9):
            panel(face, s)
        for cells in ((-9, -8), (-5, -4), (3, 4), (7, 8)):
            shield(face, list(cells))
        for s in (-1, 0):
            for h in (SHIELD_H - 1, SHIELD_H, SHIELD_H + 1):
                put_face(f, face, s, 0, h, RAILING)
                put_face(f, face, s, -1, h, None)
    # Parapet: a low coping with paired antefixes over the panels, the grille behind.
    for face in FACES:
        tops = range(-22, 23, 4) if face in ("east", "west") else (-10, -6, -2, 1, 5, 9)
        for s in FACES[face][2]:
            put_face(f, face, s, 0, ATTIC[1] + 1, slab(SMOOTH_SLAB))
            for h in (ATTIC[1] + 1, ATTIC[1] + 2):
                put_face(f, face, s, -2, h, RAILING)
        for s in tops:
            for ss in (s, s + 1 if s < 0 else s - 1):
                for d in (0, -1):
                    put_face(f, face, ss, d, ATTIC[1] + 1, ANTEFIX)
    for a in (A0 + 2, A1 - 2):
        for b in (B0 + 2, B1 - 2):
            for h in (ATTIC[1] + 1, ATTIC[1] + 2):
                f.put(a, L(h), b, RAILING)
    # Hall on the terrace under its oval glass roof, peak 49.6 m from the laser scan.
    for a in range(-7, 7):
        for b in range(-8, 9):
            ring = a in (-7, 6) or b in (-8, 8)
            if ring:
                for h in (45, 46):
                    f.put(a, L(h), b, SMOOTH)
            r = math.hypot((a + 0.5) / 6.0, b / 7.5)
            if r >= 0.86 or ring:
                f.put(a, L(47), b, slab(SMOOTH_SLAB))
                continue
            top = 47.6 + 2.0 * math.sqrt(max(0.0, 1 - (r / 0.86) ** 2))
            rib = b == 0 or abs(a + 0.5) < 0.6
            for h in range(47, int(top)):
                f.put(a, L(h), b, SMOOTH if rib and h == int(top) - 1 else GLASS)
            if top % 1 >= 0.5:
                f.put(a, L(int(top)), b, slab(SMOOTH_SLAB))


# Sculpture groups on the pillars, 6 to 18 m: rows top down, cells left to right as seen
# from in front; 1 stands one block proud of the pedestal line, 2 two blocks. Outlines traced
# from the 1838 elevation (east) and photographs (west).
GROUPS = {
    # Le Triomphe de 1810 (Cortot), south pillar of the Champs-Élysées face.
    ("east", 18): ["...11...", ".12221..", ".12221..", ".122211.", "1122211.", "1112211.",
                   "1122211.", "1222111.", "1122221.", "1222221.", "1221221.", "1111111."],
    # Le Départ des volontaires de 1792 (Rude), north pillar.
    ("east", -11): ["....11..", "..12221.", ".122221.", ".122211.", ".1122211", ".1222211",
                    "11222221", "12222221", ".1221221", ".1222221", ".122221.", ".1111111"],
    # La Paix de 1815 (Étex), north pillar of the Grande Armée face.
    ("west", -18): ["...11...", "..1221..", ".122221.", "11222211", "11222211", ".122221.",
                    ".122221.", "11222211", ".122221.", ".122221.", "11222211", "11111111"],
    # La Résistance de 1814 (Étex), south pillar.
    ("west", 11): ["....1...", "...121..", "..12221.", ".122221.", ".1222211", "11222211",
                   "1122221.", ".122222.", "11222211", "12222221", "11222211", "11111111"],
}


def groups(f):
    for (face, first), rows in GROUPS.items():
        step = -1 if face == "east" else 1
        cells = [first + step * j for j in range(8)]
        depth = {}
        for i, row in enumerate(rows):
            for s, ch in zip(cells, row):
                if ch != ".":
                    depth[(s, 17 - i)] = int(ch)
        for (s, h), n in depth.items():
            for d in range(1, n + 1):
                above = depth.get((s, h + 1), 0) >= d
                below = depth.get((s, h - 1), 0) >= d or h == 6
                if not above:
                    block = out_stairs(face, "bottom")
                elif not below:
                    block = out_stairs(face, "top")
                else:
                    block = SCULPTURE if d == n else SMOOTH
                put_face(f, face, s, d, h, block)
        # Pedestal under the group, from the base course to the dado.
        lo, hi = min(cells), max(cells)
        for s in range(lo, hi + 1):
            for h in range(BASE_TOP + 1, DADO):
                for d in (1, 2):
                    put_face(f, face, s, d, h, ASHLAR)
            put_face(f, face, s, 1, DADO, SMOOTH)
            put_face(f, face, s, 2, DADO, SMOOTH)
            put_face(f, face, s, 3, DADO, out_stairs(face, "top"))
            for d in (1, 2):
                put_face(f, face, s, d, BASE, SMOOTH)
                put_face(f, face, s, d, BASE_TOP, SMOOTH)
            put_face(f, face, s, 3, BASE, out_stairs(face, "bottom"))


def vaults(f):
    """Coffers with rosettes in the main vault: every other cell of the intrados sunk."""
    for a in range(A0 + 1, A1):
        if a % 2:
            continue
        for b in range(-MAIN_HALF - 1, MAIN_HALF + 2):
            for h in range(22, 31):
                if body_open(a, h, b):
                    continue
                if not (main_open(b, h - 1) or main_open(b - 1, h) or main_open(b + 1, h)):
                    continue
                ring = int(math.degrees(math.atan2(h + 0.5 - MAIN_SPRING, b)) // 22.5)
                if ring % 2:
                    continue
                f.put(a, L(h), b, None)
                back = (a, L(h), b + (1 if b > 0 else -1)) if abs(b) > 4 else (a, L(h + 1), b)
                f.put(*back, MEANDER)


def tomb(f):
    """The Unknown Soldier's slab mid-vault, the flame at its head toward the Champs-Élysées."""
    for a in (-2, -1, 0):
        f.put(a, FLOOR, 0, TOMB)
    for a in (0, 1, 2):
        for b in (-1, 0, 1):
            if (a, b) != (0, 0):
                f.put(a, FLOOR, b, BRONZE)
    f.points.append((1.5, 0.5, FLOOR, "netherrack"))
    f.points.append((1.5, 0.5, L(0), "fire"))
    for a in (-4, 4):
        for b in (-3, 0, 3):
            f.points.append((a + 0.5, b + 0.5, L(0), POST))
    for a in (-2, 1):
        for b in (-3, 3):
            f.points.append((a + 0.5, b + 0.5, L(0), POST))


def floodlights(f):
    """Invisible lights a block in front of each face on a 10 m grid, plus the passages: the arch
    is floodlit at night on all four sides. They go into air under the cornice's overhang."""
    face_lit = state("light", level=15, waterlogged=False)
    vault_lit = state("light", level=13, waterlogged=False)
    for h in (1, 12, 22, 33, 41):
        for b in (-20, -10, 0, 10, 20):
            f.lights.append((A1 + 1.5, b + 0.5, L(h), face_lit))
            f.lights.append((A0 - 0.5, b + 0.5, L(h), face_lit))
        for a in (-9, -1, 7):
            f.lights.append((a + 0.5, B0 - 0.5, L(h), face_lit))
            f.lights.append((a + 0.5, B1 + 1.5, L(h), face_lit))
    for a in (-6.5, 5.5):
        for h in (10, 22):
            f.lights.append((a, 0.5, L(h), vault_lit))
    for b in (-15.5, 16.5):
        f.lights.append((0.0, b, L(8), vault_lit))


def build():
    f = Frame()
    body(f)
    bands(f)
    archivolts(f)
    reliefs(f)
    attic(f)
    groups(f)
    vaults(f)
    tomb(f)
    floodlights(f)
    return f


# ---------------------------------------------------------------- frame -> model grid

def to_model(f):
    """Turn the frame onto the grid: each model column takes the frame column under its
    centre, so arches and courses keep their true sections at any angle."""
    cos_t, sin_t = math.cos(TURN), math.sin(TURN)
    columns = {}
    for (a, y, b), s in f.cells.items():
        columns.setdefault((a, b), {})[y] = s
    corners = [(a + da, b + db) for (a, b) in columns for da in (0, 1) for db in (0, 1)]
    xs = [a * cos_t - b * sin_t for a, b in corners]
    zs = [a * sin_t + b * cos_t for a, b in corners]
    # Model point of the frame origin, whole blocks in from the corner of the footprint.
    x0, z0 = -math.floor(min(xs)), -math.floor(min(zs))
    w, l = math.ceil(max(xs)) + x0, math.ceil(max(zs)) + z0
    blocks = {}
    for x in range(w):
        for z in range(l):
            dx, dz = x + 0.5 - x0, z + 0.5 - z0
            a = math.floor(dx * cos_t + dz * sin_t)
            b = math.floor(-dx * sin_t + dz * cos_t)
            for y, s in columns.get((a, b), {}).items():
                blocks[(x, y, z)] = s
    for a, b, y, block in f.points:
        x = math.floor(x0 + a * cos_t - b * sin_t)
        z = math.floor(z0 + a * sin_t + b * cos_t)
        blocks[(x, y, z)] = block
    # A light must have model blocks under it, or placement would fill the gap with stone.
    occupied = {(x, z) for x, _, z in blocks}
    for a, b, y, block in f.lights:
        px, pz = x0 + a * cos_t - b * sin_t, z0 + a * sin_t + b * cos_t
        spots = [(x, z) for x in range(math.floor(px) - 1, math.floor(px) + 2)
                 for z in range(math.floor(pz) - 1, math.floor(pz) + 2)
                 if (x, z) in occupied and (x, y, z) not in blocks
                 and any((x, yy, z) in blocks for yy in range(y))]
        if spots:
            x, z = min(spots, key=lambda c: (c[0] + 0.5 - px) ** 2 + (c[1] + 0.5 - pz) ** 2)
            blocks[(x, y, z)] = block
    connect(blocks)
    # The frame's centre: a = 0 between the faces, b = 0.5 on the arch axis.
    anchor = (x0 - 0.5 * sin_t, z0 + 0.5 * cos_t)
    return blocks, anchor


BARS_MARK = "@bars:"


def full_block(s):
    if s is None or s.startswith(WALL_MARK) or s.startswith(BARS_MARK):
        return False
    return "[" not in s or "type=double" in s


def connect(blocks):
    """Walls and bars join their neighbours, as the game would show them once updated."""
    marked = dict(blocks)
    for k, s in marked.items():
        mark = WALL_MARK if s.startswith(WALL_MARK) else BARS_MARK if s.startswith(BARS_MARK) else None
        if mark is None:
            continue
        x, y, z = k
        above = marked.get((x, y + 1, z))
        sides = {}
        for d, (dx, dz) in (("north", (0, -1)), ("south", (0, 1)), ("west", (-1, 0)),
                            ("east", (1, 0))):
            n = marked.get((x + dx, y, z + dz))
            joins = n is not None and (n.startswith(mark) or full_block(n))
            if mark == BARS_MARK:
                sides[d] = joins
            else:
                sides[d] = ("tall" if above is not None else "low") if joins else "none"
        if mark == BARS_MARK:
            blocks[k] = state(s[len(mark):], waterlogged=False, **sides)
        else:
            blocks[k] = state(s[len(mark):], up=True, waterlogged=False, **sides)


def main(argv):
    out = argv[0] if argv else os.path.join(HERE, "arc_de_triomphe.schem")
    blocks, (ax, az) = to_model(build())
    blocks = {k: (v if ":" in v else "minecraft:" + v) for k, v in blocks.items()}
    size = tuple(max(k[i] for k in blocks) + 1 for i in range(3))
    data, n = sponge(blocks, size)
    assert n <= 256, n
    with open(out, "wb") as fh:
        fh.write(data)
    print(f"{out}: {size[0]}x{size[1]}x{size[2]}, {len(blocks)} blocks, {n} states, "
          f"anchor at model ({ax:.3f}, {az:.3f})")


if __name__ == "__main__":
    main(sys.argv[1:])
