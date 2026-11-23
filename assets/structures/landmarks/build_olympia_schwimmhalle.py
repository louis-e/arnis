#!/usr/bin/env python3
"""Builds olympia_schwimmhalle_munich.schem, the Olympia-Schwimmhalle in Munich, beside this script.

    python3 assets/structures/landmarks/build_olympia_schwimmhalle.py [CACHE_DIR]

Survey downloads go to CACHE_DIR (else $ARNIS_SURVEY_CACHE, else ~/.cache/arnis-survey).
Deps: numpy, laspy[lazrs], pyproj, pillow.

Takes in the pool roof and the roof towards the Olympiahalle ("Olympia-Dach zw. -Halle und
-Schwimmbad", OSM way 117468809). The roof skin is the Bavarian laser scan (tile 689_5338) inside
the mapped outlines; the leaning mast, its back-stays, the cables down to the cone and the tie-downs
at the roof's tips were fitted to the same points and to the mapped cables, and are listed below in
EPSG:25832 metres from E 689000 N 5338000. The glazed wall follows the mapped building. The pools
follow the city's figures (50 m pool, diving pool 21 x 20 m, 5 m deep, a 10 m tower); their layout
(in line under the cone, the stand on the hill side) is read from photos and only approximate.

Data: Bayerische Vermessungsverwaltung - www.geodaten.bayern.de, CC BY 4.0;
OpenStreetMap contributors, ODbL.
"""
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import olympiapark_common as op  # noqa: E402

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "olympia_schwimmhalle_munich.schem")
TILES = [(689, 5338)]
UTM_BOX = (596, 795, 610, 848)
Z0 = 506

HALL = 227012665
ROOFS = [15805217, 117468809]
MASTS = {"lake": ((701.4, 702.4, 586.9), (726.3, 678.7, 512.2))}
CONE = (698.0, 704.0)
CABLES = [
    ("lake", (791.0, 662.4), "ground"), ("lake", (736.6, 614.4), "ground"),
    ("lake", (CONE[0] - 1.2, CONE[1]), "net"), ("lake", CONE, "net"), ("lake", (CONE[0] + 1.2, CONE[1]), "net"),
]
# Tie-downs from the roof's tips to ground anchors (mapped as cables or small concrete roofs).
EDGE_ANCHORS = [
    (118153989, (750.6, 684.4), (737.7, 704.9)), (118153993, (764.1, 708.0), (737.7, 704.9)),
    (118153987, (766.1, 778.0), (747.9, 781.2)), (117479859, (769.6, 793.2), (747.9, 781.2)),
    (117470710, (752.8, 829.7), (723.0, 817.6)), (118153988, (709.6, 844.2), (723.0, 817.6)),
    (118153991, (673.9, 646.8), (688.5, 661.2)), (269819552, (689.3, 640.8), (688.5, 661.2)),
    (118161792, (635.6, 689.8), (642.3, 699.8)), (118161796, (626.1, 706.8), (642.2, 721.0)),
    (117468800, (625.5, 727.2), (629.5, 731.5)), (117468805, (607.5, 797.1), (607.5, 805.5)),
    (117468808, (680.3, 816.0), (676.5, 818.5)), (117468810, (653.7, 796.8), (653.5, 802.5)),
    (360117188, (693.6, 825.8), (692.5, 819.5)), (360341557, (633.4, 810.3), (628.5, 814.5)),
]
# Ties 117468801, 117468806 and 360341561 are in the Olympiahalle model.
FOOTINGS = [360117189]

DECK = 513.5
STAND_TOP = 524.0
FRAME_STEEL = "light_gray_concrete"
FACADE_GLASS = "glass_pane"
LINING = "white_concrete"
TILE = "smooth_quartz"
POOL_WALL = "prismarine"
LANE = "dark_prismarine"
SEAT = "polished_andesite_stairs"
RISER = "polished_andesite"
TOWER = "smooth_stone"

# Pools as (centre E, centre N, length N-S, width E-W, depth), metres: in line along the hall, as far
# east as the glass wall allows with a 4 m deck, the stand west of them. The teaching pools lie under
# the stand and are not modelled.
POOLS = {
    "competition": (698.9, 761.8, 50.0, 21.0, 2.0),
    "diving": (700.2, 720.8, 20.0, 21.0, 5.0),
}


def interior(t, hall):
    m, g, z0 = t.m, t.g, t.z0
    inside = hall & ~op.outline(hall)
    lid = np.where(hall & np.isfinite(t.roof), np.round(t.roof - z0).astype(int) - 2, -1)
    lining = m.sid(LINING)
    for i, k in zip(*np.nonzero(lid > 0)):
        m.v[i, lid[i, k], k] = lining
    deck = int(round(DECK - z0)) - 1
    ii, kk = np.meshgrid(np.arange(g.w), np.arange(g.l), indexing="ij")
    cx, cy = g.centre(ii, kk)
    pools = {}
    for name, (e, n, length, width, depth) in POOLS.items():
        px, py = t.utm(e, n)
        mask = (np.abs(cx - px) < width / 2) & (np.abs(cy - py) < length / 2) & inside
        pools[name] = (mask, int(np.ceil(depth)), px, py)
    wet = np.zeros_like(hall)
    for mask, _, _, _ in pools.values():
        wet |= mask
    # The stand climbs west from the pool deck up the hill to the foyer at the park's level.
    sx, _ = t.utm(681.0, 750.0)
    run = (sx - cx) * 0.45
    stand = inside & ~wet & (run > 0.6)
    stand_y = deck + np.minimum(np.round(run).astype(int), int(round(STAND_TOP - DECK)))
    for i, k in zip(*np.nonzero(inside)):
        if stand[i, k]:
            for yy in range(deck, stand_y[i, k]):
                m.set(i, yy, k, RISER)
            m.set(i, stand_y[i, k], k, f"{SEAT}[facing=west,half=bottom,shape=straight,waterlogged=false]"
                  if stand_y[i, k] < deck + round(STAND_TOP - DECK) else RISER)
        elif not wet[i, k]:
            m.set(i, deck, k, TILE)
    for name, (mask, depth, px, py) in pools.items():
        rim = op.outline(mask)
        for i, k in zip(*np.nonzero(mask)):
            m.set(i, deck - depth - 1, k, POOL_WALL)
            for yy in range(deck - depth, deck + 1):
                m.set(i, yy, k, POOL_WALL if rim[i, k] else "water")
        # Sparse underwater lamps in the side walls, just under the water line.
        for i, k in zip(*np.nonzero(rim)):
            if (i + k) % 5 == 0 and m.get(i, deck - 1, k) == op.state(POOL_WALL):
                m.set(i, deck - 1, k, "sea_lantern")
        if name == "competition":
            # Lane lines on the floor every 2.5 m.
            for i, k in zip(*np.nonzero(mask & ~rim)):
                if int(cx[i, k] - px + 10.5) % 3 == 1 and abs(cy[i, k] - py) < 23:
                    m.set(i, deck - depth - 1, k, LANE)
    # Diving tower on the south side of the diving pool, under the cone: 1, 3, 5, 7.5 and 10 m.
    mask, depth, px, py = pools["diving"]
    ti, tk = g.cell(px, py - 10.0 - 3.0)
    for yy in range(deck + 1, deck + 12):
        for di in (-2, -1, 0, 1, 2):
            for dk in (0, 1, 2):
                if abs(di) == 2 and dk:
                    continue
                m.set(ti + di, yy, tk + dk, TOWER)
    for h, reach, half in ((1, 3, 1), (3, 3, 1), (5, 4, 2), (8, 5, 2), (10, 6, 3)):
        for dk in range(-reach, 0):
            for di in range(-half, half + 1):
                m.set(ti + di, deck + h, tk + dk, TOWER)
            for di in (-half - 1, half + 1):
                if dk > -reach:
                    m.set(ti + di, deck + h + 1, tk + dk, "iron_bars")
    base = np.minimum(np.floor(t.ground - z0).astype(int), deck)
    t.glass_wall(hall, base, lid + 1, FRAME_STEEL, FACADE_GLASS)
    t.light_interior(hall, lid)


def build(cache):
    t = op.Tent(cache, "schwimmhalle", TILES, UTM_BOX, Z0, ROOFS, MASTS, EDGE_ANCHORS)
    hall = t.g.polygon(t.ways[HALL][1])
    interior(t, hall)
    t.structure(CABLES, FOOTINGS)
    t.m.connect()
    return t, hall


if __name__ == "__main__":
    t, hall = build(op.cache_dir(sys.argv))
    p = t.placement(hall, 40.0, 55.0)
    t.m.save(OUT)
    op.report(t.m, OUT)
    print("placement", p)
    if os.environ.get("OLYMPIAPARK_DEBUG"):
        np.savez_compressed(os.environ["OLYMPIAPARK_DEBUG"], ground=t.ground, roof=t.roof, z0=Z0, hall=hall, x0=t.g.x0, y1=t.g.y1)
