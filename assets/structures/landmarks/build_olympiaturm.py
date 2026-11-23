#!/usr/bin/env python3
"""Builds olympiaturm_munich.schem, the Olympiaturm in Munich, beside this script.

    python3 assets/structures/landmarks/build_olympiaturm.py

One block per metre, x east, z south, layer 0 on the ground at the shaft foot. Deps: numpy.

Heights come from the Bavarian laser scan (tile 689_5338, CC BY 4.0 Bayerische
Vermessungsverwaltung - www.geodaten.bayern.de), read as a radius profile round the shaft axis,
and from the published figures (de.wikipedia, after Hoyer/Teutschbein 1969): shaft 16.50 m at
the foot tapering to 8.60 m at 145 m, telecom basket 21.10 m across from 147.25 to 167.70 m with
three galleries cantilevering 3.60 m, visitor basket 28.30 m across with levels at 178, 182,
185, 189 and 192 m, concrete to 248 m, a 19.2 m GFK antenna on top, 291 m in all.
"""
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from olympiapark_common import Model, dist_grid, erode8, outline, report  # noqa: E402

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "olympiaturm_munich.schem")

N = 31
C = 15.5  # shaft axis, a block centre; E 689848.80 N 5338840.04 (EPSG:25832)
TOP = 290

CONCRETE = "smooth_stone"
# The two baskets are painted white over the bare concrete of the shaft.
HEAD = "white_concrete"
PAINT = "white_concrete"
PANEL = "white_concrete"
WINDOW = "brown_stained_glass"
SLIT = "black_stained_glass"
VENT = "polished_blackstone"
STEEL = "iron_bars"
GRATING = "smooth_stone_slab[type=top,waterlogged=false]"
DECK = "smooth_quartz"
POST = "andesite_wall"
RED = "red_concrete"

D = dist_grid(N, N, C, C)
ANG = np.arctan2(np.arange(N)[None, :] + 0.5 - C, np.arange(N)[:, None] + 0.5 - C)


def disc(r):
    return D <= r + 0.15


def ring(r, width=1):
    d = disc(r)
    inner = d.copy()
    for _ in range(width):
        inner = erode8(inner)
    return d & ~inner


def shaft_r(h):
    # Laser fit (r 7.85 at 12 m, 6.51 at 52 m, 5.95 at 72 m) held to the published 8.60 m at 145 m.
    h = min(h, 145.0)
    return 8.25 - 0.03623 * h + 6.2e-5 * h * h


def perimeter(mask):
    """Outline cells of a disc in angular order, so bays can be counted round it."""
    xs, zs = np.nonzero(outline(mask))
    order = np.argsort(ANG[xs, zs])
    return list(zip(xs[order], zs[order]))


def bays(m, y, r, every, inside, between):
    """A facade ring: `inside` cells with an `between` mullion every `every` cells."""
    for i, (x, z) in enumerate(perimeter(disc(r))):
        m.set(int(x), y, int(z), between if i % every == 0 else inside)


def build():
    m = Model(N, TOP + 1, N)

    # Shaft, hollow, up into the hat.
    for y in range(0, 205):
        r = shaft_r(y + 0.5) if y < 146 else 4.3
        m.layer(y, ring(r, 2), CONCRETE)

    # Telecom basket 147.25-167.70 m: saucer, two galleries, roof.
    saucer = {144: 5.6, 145: 5.6, 146: 7.6, 147: 9.6, 148: 11.4, 149: 12.8, 150: 13.8, 151: 14.15}
    for y, r in saucer.items():
        m.layer(y, disc(r), HEAD if y < 151 else DECK)
    for floor in (151, 159, 167):
        m.layer(floor, disc(14.15), DECK)
        m.layer(floor, outline(disc(14.15)), HEAD)
    for g0 in (152, 160):
        body = disc(10.55)
        for y in range(g0, g0 + 7):
            m.layer(y, ring(10.55, 2), PANEL)
        # Small square windows in two staggered rows, as on the operations floors.
        for row, y in enumerate((g0 + 1, g0 + 4)):
            for i, (x, z) in enumerate(perimeter(body)):
                if (i + 2 * row) % 4 == 0:
                    m.set(int(x), y, int(z), SLIT)
        # Railing on the gallery edge, steel fascia under the slab above, posts between.
        edge = perimeter(disc(14.15))
        for i, (x, z) in enumerate(edge):
            m.set(int(x), g0, int(z), STEEL)
            m.set(int(x), g0 + 5, int(z), STEEL)
            m.set(int(x), g0 + 6, int(z), STEEL)
            if i % 6 == 0:
                for y in range(g0, g0 + 7):
                    m.set(int(x), y, int(z), POST)

    # Visitor basket: bowl, restaurant and viewing bands, open deck.
    bowl = {170: 5.6, 171: 7.0, 172: 8.6, 173: 10.0, 174: 11.2, 175: 12.2, 176: 13.0, 177: 13.6, 178: 14.0}
    for y, r in bowl.items():
        m.layer(y, disc(r), HEAD)
    for i, (x, z) in enumerate(perimeter(disc(13.6))):
        if i % 6 == 3:
            m.set(int(x), 177, int(z), VENT)
    for y in (178, 180, 184):
        m.layer(y, disc(13.6), DECK)
    for y in range(179, 190):
        m.layer(y, ring(14.15), PANEL)
    for y in (181, 182, 183, 186, 187):
        bays(m, y, 14.15, 4, WINDOW, PANEL)
    m.layer(189, disc(14.15), DECK)
    m.layer(189, outline(disc(14.15)), PANEL)
    for y, r in ((190, 14.15), (191, 14.15), (192, 13.5)):
        m.layer(y, outline(disc(r)), STEEL)
    # Kiosk drum between the decks, the upper deck on it.
    for y in (190, 191):
        bays(m, y, 9.6, 4, "glass", PANEL)
    m.layer(192, disc(10.0), DECK)
    m.layer(192, outline(disc(10.0)), PANEL)
    for y in (193, 194):
        m.layer(y, outline(disc(10.0)), STEEL)
    # Two outside stairs from the open deck up to the top one, through gaps in its railing.
    for z, x0, step, facing in ((4, 13, 1, "east"), (26, 17, -1, "west")):
        for k in range(3):
            m.set(x0 + step * k, 190 + k, z, f"polished_andesite_stairs[facing={facing},half=bottom,shape=straight,waterlogged=false]")
        zd = 5 if z == 4 else 25
        for y in (193, 194):
            m.set(x0 + step * 2, y, zd, "air")
    for y in range(193, 200):
        m.layer(y, ring(4.3, 2), PAINT)

    # The hat over the top deck.
    hat = {200: 5.4, 201: 7.0, 202: 8.4, 203: 9.8, 204: 10.0}
    for y, r in hat.items():
        m.layer(y, disc(r), PAINT)
    for a in (45, 135, 225, 315):
        x = int(C + 6.5 * math.cos(math.radians(a)))
        z = int(C + 6.5 * math.sin(math.radians(a)))
        m.set(x, 205, z, POST)
        m.set(x, 206, z, "white_concrete")
        m.set(x, 207, z, "white_concrete")

    # Upper shaft with its two steel galleries, then the red cage at the top of the concrete.
    for y in range(205, 247):
        r = 3.15 if y < 222 else 2.15
        m.layer(y, disc(r), PAINT)
    for floor, r in ((219, 4.6), (226, 3.8)):
        m.layer(floor, disc(r), GRATING, only_air=True)
        m.layer(floor + 1, outline(disc(r)), STEEL)
        m.layer(floor + 2, outline(disc(r)), STEEL)
    m.layer(247, disc(3.5), RED)
    m.layer(248, outline(disc(3.5)), RED)

    # Steel antenna carrier with red cages, then the GFK cylinder in day-marker bands.
    for y in range(249, 268):
        m.layer(y, disc(1.3), RED if y < 251 or 258 <= y < 264 else PAINT)
    for y in (252, 256, 264, 267):
        m.layer(y, outline(disc(2.3)), RED)
    for y in range(268, 288):
        m.set(15, y, 15, PAINT if (y - 268) // 5 % 2 == 0 else RED)
    m.layer(288, outline(disc(1.3)), RED)
    m.set(15, 288, 15, RED)
    m.set(15, 289, 15, RED)
    m.set(15, 290, 15, "lightning_rod[facing=up,powered=false,waterlogged=false]")

    lights(m)
    m.connect()
    return m


def lights(m):
    """Lit restaurant and viewing floors, red aviation lights on the hat, the cages and the tip."""
    glow = "light[level=15,waterlogged=false]"
    for y, r, every, block in ((183, 12.0, 5, "sea_lantern"), (188, 12.0, 5, "sea_lantern"),
                               (181, 9.0, 6, glow), (186, 9.0, 6, glow)):
        for i, (x, z) in enumerate(perimeter(disc(r))):
            if i % every == 0 and m.get(int(x), y, int(z)) == "minecraft:air":
                m.set(int(x), y, int(z), block)
    red = "redstone_torch[lit=true]"
    c = int(C)
    for y, r in ((205, 9), (249, 3), (253, 2), (257, 2), (265, 2), (268, 2), (289, 1)):
        for dx, dz in ((r, 0), (-r, 0), (0, r), (0, -r)):
            if m.get(c + dx, y, c + dz) == "minecraft:air":
                m.set(c + dx, y, c + dz, red)


if __name__ == "__main__":
    m = build()
    m.save(OUT)
    report(m, OUT)
    for n, s in m.counts()[:30]:
        print(f"  {n:7d} {s}")
