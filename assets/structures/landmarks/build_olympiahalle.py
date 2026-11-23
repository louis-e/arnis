#!/usr/bin/env python3
"""Builds olympiahalle_munich.schem, the Olympiahalle in Munich, beside this script.

    python3 assets/structures/landmarks/build_olympiahalle.py [CACHE_DIR]

Survey downloads go to CACHE_DIR (else $ARNIS_SURVEY_CACHE, else ~/.cache/arnis-survey).
Deps: numpy, laspy[lazrs], pyproj, pillow.

Takes in the hall roof and the connecting roofs on to the Olympiastadion's NE tip (OSM ways
302726370, 117477621, 117470701); the stadium model owns way 25001469, the Schwimmhalle model the
roof towards the pool (117468809). The roof skin is the Bavarian laser scan (tiles 689_5338 and
689_5339) inside the mapped roof outlines; masts, guy and suspension cables and the edge anchors
were fitted to the same points and are listed below in EPSG:25832 metres from E 689000 N 5338000.
The glazed wall follows the mapped building, the seating bowl the mapped indoor areas.

Data: Bayerische Vermessungsverwaltung - www.geodaten.bayern.de, CC BY 4.0;
OpenStreetMap contributors, ODbL.
"""
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import olympiapark_common as op  # noqa: E402

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "olympiahalle_munich.schem")
TILES = [(689, 5338), (689, 5339)]
UTM_BOX = (356, 684, 796, 1054)
Z0 = 506

HALL = 303099272
ROOFS = [15805226, 302726370, 117477621, 117470701]
ARENA, STAGE = 360126404, 360126408

# (head, foot) of each mast, fitted to the laser points.
MASTS = {
    "north_west": ((563.2, 950.6, 578.6), (569.3, 961.5, 512.6)),
    "north_east": ((620.9, 932.8, 578.4), (621.7, 945.5, 511.7)),
    "connector": ((422.4, 949.8, 568.0), (428.7, 936.7, 518.3)),
}
# Cables from a mast head: to a ground anchor ("ground") or to a hanging point in the net ("net").
CABLES = [
    ("north_west", (563.3, 1008.3), "ground"), ("north_west", (560.1, 1006.5), "ground"),
    ("north_west", (613.3, 1008.3), "ground"),
    ("north_west", (555.3, 912.3), "net"), ("north_west", (563.3, 934.3), "net"),
    ("north_east", (614.5, 1009.5), "ground"), ("north_east", (656.5, 977.5), "ground"),
    ("north_east", (650.5, 979.5), "ground"),
    ("north_east", (604.5, 897.5), "net"), ("north_east", (612.5, 921.5), "net"),
    ("connector", (400.5, 1049.8), "ground"),
    ("connector", (452.5, 895.8), "ground"), ("connector", (422.5, 889.8), "ground"),
    ("connector", (448.5, 903.8), "net"),
]
# Edge anchors (mapped as small concrete roofs) and the net edge point each one holds down.
EDGE_ANCHORS = [
    (117468801, (667.1, 849.2), (664.5, 839.5)), (117468806, (664.0, 842.2), (665.5, 839.5)),
    (117470696, (451.5, 905.9), (454.5, 907.5)), (117470697, (393.7, 921.7), (396.5, 916.5)),
    (117470700, (378.9, 939.9), (392.5, 937.5)), (117470703, (393.1, 923.9), (396.5, 925.5)),
    (117470705, (392.7, 967.1), (398.5, 969.5)), (117470707, (451.6, 895.5), (464.5, 905.5)),
    (117470713, (447.4, 901.9), (443.5, 906.5)), (117523417, (429.5, 870.2), (419.5, 875.5)),
    (118239000, (568.0, 808.0), (570.5, 816.5)), (118241447, (486.1, 849.3), (488.5, 855.5)),
    (118241448, (535.6, 819.6), (538.5, 827.5)), (118241449, (469.1, 868.7), (475.5, 870.5)),
    (118241450, (460.3, 902.5), (464.5, 905.5)), (118241451, (507.1, 828.7), (505.5, 838.5)),
    (360117190, (521.8, 835.1), (522.5, 839.5)), (360117191, (561.1, 827.3), (563.5, 829.5)),
    (360117192, (498.1, 831.8), (505.5, 840.5)), (360117193, (552.9, 829.4), (553.5, 832.5)),
    (360117194, (589.8, 814.1), (590.5, 817.5)), (360341546, (502.0, 951.0), (504.5, 945.5)),
    (360341552, (562.0, 961.6), (559.5, 956.5)), (360341561, (628.1, 813.4), (635.5, 814.5)),
    (117470711, (593.5, 968.9), (589.5, 954.5)),
]
# Footings mapped without a visible cable of their own (mast feet, back-stay anchors).
FOOTINGS = [117470709, 117470712, 117470714, 117524350, 360341549, 360341554, 360341560,
            360341564, 117479853, 117470698, 117470708, 118241446, 117523419, 117477617]

ARENA_FLOOR = 511.0
BOWL_TOP = 527.0
CONCOURSE = 519.5

FRAME_STEEL = "gray_concrete"
FACADE_GLASS = "gray_stained_glass_pane"
LINING = "light_gray_wool"
SEAT = "polished_deepslate_stairs"
RISER = "polished_andesite"
FLOOR = "smooth_stone"
FILL = "stone"
ARENA_TOP = "gray_concrete"
STAGE_TOP = "black_concrete"


def interior(t, hall):
    """Lining under the net, concourse, seating bowl on the mapped blocks, arena, glass wall."""
    m, g, ways, z0 = t.m, t.g, t.ways, t.z0
    inside = hall & ~op.outline(hall)
    lid = np.where(hall & np.isfinite(t.roof), np.round(t.roof - z0).astype(int) - 2, -1)
    lining = m.sid(LINING)
    for i, k in zip(*np.nonzero(lid > 0)):
        m.v[i, lid[i, k], k] = lining
    concourse = int(round(CONCOURSE - z0)) - 1
    floor_y = int(round(ARENA_FLOOR - z0)) - 1
    arena = g.polygon(ways[ARENA][1]) & hall
    stage = g.polygon(ways[STAGE][1]) & hall
    seats = np.zeros_like(hall)
    for wid, (tags, r) in ways.items():
        if tags.get("indoor") == "area" and wid not in (ARENA, STAGE):
            poly = g.polygon(r)
            if 0 < poly.sum() and (poly & hall).sum() > 0.9 * poly.sum():
                seats |= poly
    pit = arena | stage
    bowl = op.erode(op.dilate(seats | pit, 2), 2) & inside
    din = op.distance_from(pit)
    dout = op.distance_from(~bowl)
    frac = np.clip(din / np.maximum(din + dout, 1), 0, 1)
    rows = floor_y + 1 + np.round(frac * (BOWL_TOP - ARENA_FLOOR - 1)).astype(int)
    gx, gz = np.gradient(np.where(np.isfinite(din), din, 0.0))
    for i, k in zip(*np.nonzero(inside)):
        if pit[i, k]:
            m.set(i, floor_y, k, STAGE_TOP if stage[i, k] else ARENA_TOP)
        elif bowl[i, k]:
            y = rows[i, k]
            for yy in range(floor_y, y):
                m.set(i, yy, k, RISER)
            if din[i, k] <= 1:
                m.set(i, y, k, RISER)
            else:
                if abs(gx[i, k]) >= abs(gz[i, k]):
                    facing = "east" if gx[i, k] > 0 else "west"
                else:
                    facing = "south" if gz[i, k] > 0 else "north"
                m.set(i, y, k, f"{SEAT}[facing={facing},half=bottom,shape=straight,waterlogged=false]")
        else:
            # Concourse at the park's level on solid ground; a lobby opens where the park lies lower.
            m.set(i, concourse, k, FLOOR)
            grade = min(int(np.floor(t.ground[i, k] - z0)) - 1, concourse - 1)
            for yy in range(floor_y, grade):
                m.set(i, yy, k, FILL)
            if grade < concourse - 1:
                m.set(i, grade, k, FLOOR)
    base = np.minimum(np.floor(t.ground - z0).astype(int), concourse)
    t.glass_wall(hall, base, lid + 1, FRAME_STEEL, FACADE_GLASS)
    t.light_interior(hall, lid)


def build(cache):
    t = op.Tent(cache, "halle", TILES, UTM_BOX, Z0, ROOFS, MASTS, EDGE_ANCHORS)
    hall = t.g.polygon(t.ways[HALL][1])
    interior(t, hall)
    t.structure(CABLES, FOOTINGS)
    t.m.connect()
    return t, hall


if __name__ == "__main__":
    t, hall = build(op.cache_dir(sys.argv))
    p = t.placement(hall, 60.0, 50.0)
    t.m.save(OUT)
    op.report(t.m, OUT)
    print("placement", p)
    if os.environ.get("OLYMPIAPARK_DEBUG"):
        np.savez_compressed(os.environ["OLYMPIAPARK_DEBUG"], ground=t.ground, roof=t.roof, z0=Z0, hall=hall, x0=t.g.x0, y1=t.g.y1)
