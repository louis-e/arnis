#!/usr/bin/env python3
"""Builds the fountain schematics (fountain_*.schem) beside this script.

    python3 assets/structures/build_fountains.py [--dump]

Each model is drawn in layers from its lowest one, and `sink` of them lie below the ground
it stands on: 2 for a basin let into the ground (floor, then water level with the paving),
1 for one on its own floor, 0 for one on the ground. src/structures/fountain.rs carries the
same numbers.

Water rules, which fountain.rs checks again: still water (a source) only touches stone, other
water or the ground, so nothing runs off when a block next to it is updated. Jets, spouts and
curtains are falling water with no source above, which at most dries up.
"""
import gzip
import io
import math
import os
import re
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SCHEMATIC_RS = os.path.join(HERE, "..", "..", "src", "structures", "schematic.rs")
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


def sponge(model):
    xs, ys, zs = zip(*model.blocks)
    w, h, l = max(xs) + 1, max(ys) + 1, max(zs) + 1
    palette = {"minecraft:air": 0}
    data = bytearray()
    for y in range(h):
        for z in range(l):
            for x in range(w):
                state = model.blocks.get((x, y, z), "minecraft:air")
                data += _varint(palette.setdefault(state, len(palette)))
    blocks = {
        "Palette": (10, {k: (3, v) for k, v in palette.items()}),
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


# ---------------------------------------------------------------- drawing


def state(name, **props):
    name = name if ":" in name else "minecraft:" + name
    if not props:
        return name
    inner = ",".join(f"{k}={str(v).lower()}" for k, v in sorted(props.items()))
    return f"{name}[{inner}]"


SOURCE = state("water", level=0)
FALLING = state("water", level=8)

DIRS = {"north": (0, -1), "south": (0, 1), "west": (-1, 0), "east": (1, 0)}
LEFT = {"north": "west", "west": "south", "south": "east", "east": "north"}


def dir_of(dx, dz):
    return next(d for d, v in DIRS.items() if v == (dx, dz))


def sign(v):
    return (v > 0) - (v < 0)


def slab(name, wet=False):
    return state(name, type="bottom", waterlogged=wet)


def wall(name):
    return state(name, east="none", north="none", south="none", west="none", up=True,
                 waterlogged=False)


def stairs(name, facing, shape="straight", half="bottom"):
    return state(name, facing=facing, half=half, shape=shape, waterlogged=False)


def corner(name, a, b, kind, half="bottom"):
    """Stairs turning a corner whose two sides face `a` and `b`."""
    return stairs(name, a, f"{kind}_{'left' if b == LEFT[a] else 'right'}", half)


def rod(facing):
    return state("lightning_rod", facing=facing, powered=False, waterlogged=False)


class Model:
    def __init__(self, name, sink, kind="basin"):
        self.name, self.sink, self.kind = name, sink, kind
        self.blocks = {}

    def put(self, cells, y, block):
        for x, z in cells:
            self.blocks[(x, y, z)] = block


def octagon(n, cut):
    return {(x, z) for x in range(n) for z in range(n)
            if min(x, n - 1 - x) + min(z, n - 1 - z) >= cut}


def disc(n):
    c, r = (n - 1) / 2, n / 2 - 0.1
    return {(x, z) for x in range(n) for z in range(n) if math.hypot(x - c, z - c) <= r}


def ring_of(shape):
    """The cells of `shape` with a side open to the outside: the rim."""
    return {(x, z) for x, z in shape
            if any(n not in shape for n in ((x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)))}


def around(c, r):
    return {(c[0] + dx, c[1] + dz) for dx in range(-r, r + 1) for dz in range(-r, r + 1)}


def square_ring(c, r):
    return around(c, r) - around(c, r - 1) if r else {c}


def sides(c, r=1):
    return {(c[0] + r, c[1]), (c[0] - r, c[1]), (c[0], c[1] + r), (c[0], c[1] - r)}


def at(c, offsets):
    return {(c[0] + dx, c[1] + dz) for dx, dz in offsets}


def ring_offsets(r, diagonal):
    return [(r, 0), (-r, 0), (0, r), (0, -r)] + [(dx * diagonal, dz * diagonal)
                                               for dx in (-1, 1) for dz in (-1, 1)]


def coping(m, rim, water, y, name, fallback):
    """Stairs on a rim, stepping up from outside to a lip over the water; slabs where the
    rim does not run straight beside it."""
    for x, z in rim:
        inward = [d for d, (dx, dz) in DIRS.items() if (x + dx, z + dz) in water]
        diagonal = [(dx, dz) for dx in (-1, 1) for dz in (-1, 1) if (x + dx, z + dz) in water]
        if len(inward) == 1:
            block = stairs(name, inward[0])
        elif len(inward) == 2 and DIRS[inward[0]][0] != -DIRS[inward[1]][0]:
            block = corner(name, inward[0], inward[1], "inner")
        elif not inward and len(diagonal) == 1:
            dx, dz = diagonal[0]
            block = corner(name, dir_of(dx, 0), dir_of(0, dz), "outer")
        else:
            block = slab(fallback)
        m.put({(x, z)}, y, block)


def moulded_ring(m, c, r, y, name, half):
    """A square ring of stairs `r` out from `c`, every one turned to the middle."""
    for x, z in square_ring(c, r):
        dx, dz = x - c[0], z - c[1]
        if abs(dx) == r and abs(dz) == r:
            block = corner(name, dir_of(-sign(dx), 0), dir_of(0, -sign(dz)), "outer", half)
        elif abs(dx) == r:
            block = stairs(name, dir_of(-sign(dx), 0), half=half)
        else:
            block = stairs(name, dir_of(0, -sign(dz)), half=half)
        m.put({(x, z)}, y, block)


def sunk_basin(m, shape, rim_block, stair, slab_name, floor="smooth_stone"):
    """A basin let into the ground: floor below, water level with the paving, a stepped
    coping above."""
    rim = ring_of(shape)
    water = shape - rim
    m.put(shape, 0, state(floor))
    m.put(rim, 1, state(rim_block))
    m.put(water, 1, SOURCE)
    coping(m, rim, water, 2, stair, slab_name)
    return rim


def small_bowl(m, c, y, stair, centre=SOURCE):
    """A 3x3 bowl on a stem: a moulded underside curving in to it, water in the middle."""
    moulded_ring(m, c, 1, y, stair, "top")
    m.put({c}, y, centre)


def wide_bowl(m, c, y, stair, belly, lip, centre):
    """A 5x5 bowl on a stem: a moulded underside and a solid belly, then a lip holding a
    ring of water round what rises from the middle."""
    moulded_ring(m, c, 2, y, stair, "top")
    m.put(square_ring(c, 1), y, state(belly))
    m.put({c}, y, centre)
    m.put(square_ring(c, 2), y + 1, slab(lip))
    m.put(square_ring(c, 1), y + 1, SOURCE)


def spouts(m, c, y, r=1):
    """Brass spouts on four sides of a column, each pouring into the water below."""
    for (x, z) in sides(c, r):
        m.put({(x, z)}, y, rod(dir_of(sign(x - c[0]), sign(z - c[1]))))
        for fall in range(2, y):
            m.put({(x, z)}, fall, FALLING)


# ---------------------------------------------------------------- models


def drinking_post():
    # A cast-iron drinking post with a brass tap over its bowl.
    m = Model("fountain_drinking_post", 0, "drinking")
    m.put({(0, 0)}, 0, wall("blackstone_wall"))
    m.put({(0, 0)}, 1, slab("polished_blackstone_slab"))
    m.put({(0, 1)}, 0, state("water_cauldron", level=3))
    m.put({(0, 1)}, 1, rod("south"))
    return m


def drinking_stone():
    # A stone drinking fountain or wall spout: a block with a tap and a basin in front.
    m = Model("fountain_drinking_stone", 0, "drinking")
    m.put({(0, 0)}, 0, state("chiseled_stone_bricks"))
    m.put({(0, 0)}, 1, wall("stone_brick_wall"))
    m.put({(0, 1)}, 0, state("water_cauldron", level=3))
    m.put({(0, 1)}, 1, state("tripwire_hook", attached=False, facing="south", powered=False))
    return m


def bowl3():
    # The smallest basin: a stone frame stepping up to a lip round a pool of water.
    m = Model("fountain_bowl3", 1)
    c = (1, 1)
    m.put(around(c, 1), 0, state("stone_bricks"))
    moulded_ring(m, c, 1, 1, "stone_brick_stairs", "bottom")
    m.put({c}, 1, SOURCE)
    return m


def pedestal3():
    # A stone bowl on a stem, bubbling in the middle.
    m = Model("fountain_pedestal3", 1)
    c = (1, 1)
    m.put(around(c, 1), 0, state("stone_bricks"))
    m.put({c}, 1, wall("stone_brick_wall"))
    small_bowl(m, c, 2, "stone_brick_stairs")
    m.put({c}, 3, FALLING)
    return m


def trough5x3():
    # A village trough fountain: stone trough, a post at one end with a brass spout.
    m = Model("fountain_trough5x3", 1)
    shape = {(x, z) for x in range(5) for z in range(3)}
    rim = ring_of(shape)
    m.put(shape, 0, state("polished_andesite"))
    m.put(rim, 1, state("polished_andesite"))
    m.put(shape - rim, 1, SOURCE)
    m.put({(0, 1)}, 2, wall("andesite_wall"))
    m.put({(0, 1)}, 3, wall("andesite_wall"))
    m.put({(0, 1)}, 4, state("polished_andesite"))
    m.put({(0, 1)}, 5, slab("polished_andesite_slab"))
    m.put({(1, 1)}, 3, rod("east"))
    m.put({(1, 1)}, 2, FALLING)
    return m


def octagon5():
    # A small octagonal basin with a column, four spouts and a capital.
    m = Model("fountain_octagon5", 2)
    c = (2, 2)
    sunk_basin(m, octagon(5, 1), "smooth_sandstone", "smooth_sandstone_stairs",
               "cut_sandstone_slab", "smooth_sandstone")
    m.put({c}, 1, state("chiseled_sandstone"))
    m.put({c}, 2, wall("sandstone_wall"))
    m.put({c}, 3, wall("sandstone_wall"))
    m.put({c}, 4, state("chiseled_sandstone"))
    m.put({c}, 5, wall("sandstone_wall"))
    spouts(m, c, 3)
    return m


def tiered7():
    # Two tiers: an octagonal basin and a stone bowl on a stem, bubbling in the middle.
    m = Model("fountain_tiered7", 2)
    c = (3, 3)
    sunk_basin(m, octagon(7, 2), "stone_bricks", "stone_brick_stairs", "stone_brick_slab")
    m.put({c}, 1, state("chiseled_stone_bricks"))
    m.put({c}, 2, wall("stone_brick_wall"))
    small_bowl(m, c, 3, "stone_brick_stairs")
    m.put({c}, 4, FALLING)
    return m


def column7():
    # An old-town fountain: octagonal basin, a column with four spouts and a capital.
    m = Model("fountain_column7", 2)
    c = (3, 3)
    sunk_basin(m, octagon(7, 2), "stone", "stone_stairs", "stone_slab")
    m.put({c}, 1, state("chiseled_stone_bricks"))
    for y in (2, 3, 4):
        m.put({c}, y, wall("stone_brick_wall"))
    m.put({c}, 5, state("chiseled_stone_bricks"))
    m.put({c}, 6, wall("stone_brick_wall"))
    spouts(m, c, 3)
    return m


def square9():
    # A square limestone basin with corner posts, a raised middle basin and a bowl above it.
    m = Model("fountain_square9", 2)
    c = (4, 4)
    shape = {(x, z) for x in range(9) for z in range(9)}
    sunk_basin(m, shape, "polished_diorite", "polished_diorite_stairs", "polished_diorite_slab")
    corners = {(0, 0), (8, 0), (0, 8), (8, 8)}
    m.put(corners, 2, state("quartz_pillar", axis="y"))
    m.put(corners, 3, slab("smooth_quartz_slab"))
    inner = around(c, 2)
    m.put(inner, 1, state("polished_diorite"))
    m.put(square_ring(c, 2), 2, state("polished_diorite"))
    m.put(square_ring(c, 1), 2, SOURCE)
    coping(m, square_ring(c, 2), square_ring(c, 1), 3, "polished_diorite_stairs",
           "polished_diorite_slab")
    m.put({c}, 2, state("chiseled_quartz_block"))
    m.put({c}, 3, state("quartz_pillar", axis="y"))
    small_bowl(m, c, 4, "smooth_quartz_stairs")
    return m


def round9():
    # A round marble basin: a column with four spouts carrying a bronze figure.
    m = Model("fountain_round9", 2)
    c = (4, 4)
    sunk_basin(m, disc(9), "smooth_quartz", "smooth_quartz_stairs", "smooth_quartz_slab")
    m.put({c}, 1, state("chiseled_quartz_block"))
    m.put({c}, 2, state("quartz_pillar", axis="y"))
    m.put({c}, 3, state("quartz_pillar", axis="y"))
    m.put({c}, 4, state("chiseled_quartz_block"))
    m.put({c}, 5, state("waxed_exposed_cut_copper"))
    spouts(m, c, 3)
    return m


def obelisk11():
    # A large basin with chamfered corners, a stepped plinth with four spouts and an obelisk.
    m = Model("fountain_obelisk11", 2)
    c = (5, 5)
    sunk_basin(m, octagon(11, 1), "smooth_sandstone", "smooth_sandstone_stairs",
               "cut_sandstone_slab")
    for y in (1, 2, 3):
        m.put(around(c, 1), y, state("smooth_sandstone"))
    moulded_ring(m, c, 1, 4, "smooth_sandstone_stairs", "bottom")
    m.put({c}, 4, state("chiseled_sandstone"))
    m.put({c}, 5, state("smooth_sandstone"))
    m.put({c}, 6, state("smooth_sandstone"))
    m.put({c}, 7, wall("sandstone_wall"))
    spouts(m, c, 3, r=2)
    return m


def jets13():
    # A round plaza basin on a dark floor: a wide bowl on a stem and a ring of eight jets.
    m = Model("fountain_jets13", 2)
    c = (6, 6)
    sunk_basin(m, disc(13), "polished_andesite", "polished_andesite_stairs",
               "polished_andesite_slab", "polished_deepslate")
    m.put({c}, 1, state("polished_andesite"))
    m.put({c}, 2, wall("andesite_wall"))
    wide_bowl(m, c, 3, "polished_andesite_stairs", "polished_andesite",
              "polished_andesite_slab", state("polished_andesite"))
    m.put({c}, 4, SOURCE)
    m.put({c}, 5, FALLING)
    m.put(at(c, ring_offsets(4, 3)), 2, FALLING)
    return m


def tiered13():
    # Three tiers in an octagonal basin: a wide bowl, a small one above it, bubbling on top.
    m = Model("fountain_tiered13", 2)
    c = (6, 6)
    sunk_basin(m, octagon(13, 4), "stone", "stone_stairs", "stone_slab")
    m.put({c}, 1, state("chiseled_stone_bricks"))
    m.put({c}, 2, wall("stone_brick_wall"))
    wide_bowl(m, c, 3, "stone_stairs", "smooth_stone", "stone_slab", wall("stone_brick_wall"))
    m.put({c}, 4, wall("stone_brick_wall"))
    m.put({c}, 5, wall("stone_brick_wall"))
    small_bowl(m, c, 6, "stone_stairs")
    m.put({c}, 7, FALLING)
    return m


def grand17():
    # A grand round basin: a raised inner basin, two bowls on a column, eight jets around.
    m = Model("fountain_grand17", 2)
    c = (8, 8)
    shape = disc(17)
    sunk_basin(m, shape, "stone_bricks", "stone_brick_stairs", "stone_brick_slab")
    dist = {p: math.hypot(p[0] - c[0], p[1] - c[1]) for p in shape}
    inner = {p for p, d in dist.items() if d <= 3.3}
    inner_rim = {p for p in inner if dist[p] > 2.3}
    inner_water = inner - inner_rim - {c}
    m.put(inner, 1, state("stone_bricks"))
    m.put(inner_rim, 2, state("stone_bricks"))
    m.put(inner_water, 2, SOURCE)
    coping(m, inner_rim, inner_water, 3, "stone_brick_stairs", "stone_brick_slab")
    m.put({c}, 2, state("chiseled_stone_bricks"))
    for y in (3, 4):
        m.put({c}, y, wall("stone_brick_wall"))
    wide_bowl(m, c, 5, "stone_brick_stairs", "stone_bricks", "stone_brick_slab",
              wall("stone_brick_wall"))
    m.put({c}, 6, wall("stone_brick_wall"))
    m.put({c}, 7, wall("stone_brick_wall"))
    small_bowl(m, c, 8, "stone_brick_stairs")
    m.put({c}, 9, FALLING)
    m.put(at(c, ring_offsets(6, 4)), 2, FALLING)
    return m


MODELS = [drinking_post, drinking_stone, bowl3, pedestal3, trough5x3, octagon5, tiered7, column7,
          square9, round9, obelisk11, jets13, tiered13, grand17]

# ---------------------------------------------------------------- checks

FACES = {"north": (0, 0, -1), "south": (0, 0, 1), "west": (-1, 0, 0), "east": (1, 0, 0)}
RIGHT = {v: k for k, v in LEFT.items()}
FLAT = list(FACES.values())
DOWN = (0, -1, 0)


def is_water(s):
    return s == "minecraft:water" or s.startswith("minecraft:water[")


def is_source(s):
    return s in (SOURCE, "minecraft:water") or "waterlogged=true" in s


def props(s):
    return dict(kv.split("=") for kv in s[s.index("[") + 1:-1].split(",")) if "[" in s else {}


def full_faces(s):
    """The sides of a block that water held in it cannot pass."""
    p = props(s)
    if "_slab" in s:
        return {"bottom": {DOWN}, "top": set()}.get(p.get("type"), set(FLAT) | {DOWN})
    if "_stairs" in s:
        faces = {DOWN} if p["half"] == "bottom" else set()
        backs = {"straight": [p["facing"]], "inner_left": [p["facing"], LEFT[p["facing"]]],
                 "inner_right": [p["facing"], RIGHT[p["facing"]]]}.get(p["shape"], [])
        return faces | {FACES[b] for b in backs}
    return set()


def waterloggable(s):
    return any(k in s for k in ("_slab", "_stairs", "_wall", "lightning_rod"))


def check(m, settled=True, mapped=True):
    """Water let loose by any change beside it stays over the model, the way Minecraft would
    run it, kept to the worst case; with `settled`, what flows already lands in still water."""
    blocks = m.blocks
    footprint = {(x, z) for x, _, z in blocks}

    def cell(x, y, z):
        if (x, y, z) in blocks:
            return blocks[(x, y, z)]
        return "ground" if y < (0 if (x, z) in footprint else m.sink) else "minecraft:air"

    def opens(c):
        s = cell(*c)
        return s == "minecraft:air" or (is_water(s) and not is_source(s))

    def held(x, y, z):
        s = cell(x, y, z)
        if is_source(s):
            return s
        # Two sources running into it and something solid under it waterlog it.
        wet = 0
        for dx, _, dz in FLAT:
            n = cell(x + dx, y, z + dz)
            wet += is_source(n) and (-dx, 0, -dz) not in full_faces(n)
        below = cell(x, y - 1, z)
        if waterloggable(s) and wet >= 2 and below != "minecraft:air" and not is_water(below):
            return s.replace("waterlogged=false", "waterlogged=true")
        return None

    reach = {}
    todo = []
    for (x, y, z) in blocks:
        s = held(x, y, z)
        if s is None:
            continue
        shut = full_faces(s) if not is_water(s) else set()
        for d in [DOWN] + FLAT:
            n = (x + d[0], y + d[1], z + d[2])
            if d not in shut and opens(n):
                assert not settled, f"{m.name}: still water opens at {n}"
                todo.append((n, 0 if d == DOWN else 1))
    while todo:
        c, dist = todo.pop()
        if reach.get(c, 99) <= dist:
            continue
        reach[c] = dist
        assert (c[0], c[2]) in footprint, f"{m.name}: water runs out at {c}"
        below = (c[0], c[1] - 1, c[2])
        if opens(below):
            todo.append((below, 0))
        elif not is_source(cell(*below)) and dist < 7:
            todo.extend(((c[0] + dx, c[1], c[2] + dz), dist + 1) for dx, _, dz in FLAT
                        if opens((c[0] + dx, c[1], c[2] + dz)))
    if settled:
        for (x, y, z), s in blocks.items():
            if is_water(s) and not is_source(s):
                below = y - 1
                while is_water(cell(x, below, z)) and not is_source(cell(x, below, z)):
                    below -= 1
                assert is_source(cell(x, below, z)), f"{m.name}: water at {(x, y, z)} hangs"
    if not mapped:
        return
    names = set(re.findall(r'^\s*"([a-z_]+)"', open(SCHEMATIC_RS).read(), re.M))
    names |= {n for line in re.findall(r'^\s*("[a-z_]+"(?:\s*\|\s*"[a-z_]+")+)',
                                        open(SCHEMATIC_RS).read(), re.M)
              for n in re.findall(r'"([a-z_]+)"', line)}
    for s in set(blocks.values()):
        base = s.split("[")[0].split(":")[1]
        assert base in names, f"{m.name}: {base} has no mapping in schematic.rs"


def dump(m):
    xs, ys, zs = zip(*m.blocks)
    w, h, l = max(xs) + 1, max(ys) + 1, max(zs) + 1
    print(f"== {m.name}  {w}x{l}, {h} high, sink {m.sink}")
    glyphs = {}
    for y in range(h):
        print(f"  y{y - m.sink:+d}")
        for z in range(l):
            row = ""
            for x in range(w):
                s = m.blocks.get((x, y, z))
                if s is None:
                    row += "."
                elif s == SOURCE:
                    row += "~"
                elif is_water(s):
                    row += "|" if "level=8" in s else ","
                else:
                    base = s.split("[")[0].split(":")[1]
                    g = glyphs.setdefault(base, "ABCDEFGHIJKLMNOPQRSTUVWXYZ"[len(glyphs)])
                    row += g.lower() if "waterlogged=true" in s else g
            print("    " + row)
    print("  " + ", ".join(f"{g}={b}" for b, g in glyphs.items()))


def main(argv):
    for build in MODELS:
        m = build()
        check(m)
        if "--dump" in argv:
            dump(m)
        with open(os.path.join(HERE, m.name + ".schem"), "wb") as f:
            f.write(sponge(m))


if __name__ == "__main__":
    main(sys.argv[1:])
