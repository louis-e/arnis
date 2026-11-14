"""Commuter and regional trains: an S-Bahn EMU, a regional EMU and a double-deck coach,
each as a cab car and a middle car.

Lengths, widths, floor and roof heights, window bands, bogies and pantographs follow
`src/element_processing/train_bodies.rs`, so the carriages couple, meet the platforms and
clear tunnels and the catenary. Rust's carriage frame has +z ahead; here the front is at
-z, so a cab car has its cab at -z and a Rust z becomes -z. Every end without a cab gets a
gangway to the neighbour, reaching 0.2 m past +-length/2 to meet the neighbour's; a cab's
front ends at -length/2.

Door positions are the look's, not Rust's (they only show): the regional EMU's and the
double-decker's doors stand just inboard of the bogies, at the low floor, as on the real
trains. Riders' eyes are at Rust's door spots, and nothing stands inside the carriage in
front of them: window bands stand outside the body, and thin fillers hide the inner face of
each raked windscreen.

The bodies are built from bays (an end, a door, the windows between two doors), painted
relative to their own box, so alike bays share one spot of the texture.
"""
import math

from kit import (
    CHROME_DARK, DRL, GLASS_DARK, GLASS_HI, GLASS_LIGHT, GLASS_MID, GLOSS, HOUSING_DARK, LENS,
    STEEL, TAIL_BRIGHT, TAIL_DARK, TRIM, UNDER, Box, Model, Paint, hexc, mix, pair,
)

M = 16.0  # units to a metre
RIDER_EYE = 2.12 * M
# Carriages couple 0.4 m apart (COUPLING_M in trains.rs): bellows, drawbars and buffers on a
# coupled end reach 0.2 m past the length, to meet the neighbour's halfway.
REACH = 0.2 * M

# ---------------------------------------------------------------- colours

FRAME = hexc("#2c2e33")
FRAME_HI = hexc("#44474e")
FRAME_LO = hexc("#1d1e22")
WHEEL = hexc("#3a3d42")
WHEEL_DISC = hexc("#2a2c30")
TREAD = hexc("#9ba0a7")
AXLE_CAP = hexc("#6a6f77")
AXLE_CAP_HI = hexc("#8b9098")
RUBBER = hexc("#19191c")
RUBBER_HI = hexc("#2b2b30")
BELLOWS = hexc("#212124")
BELLOWS_HI = hexc("#303136")
EQUIP = hexc("#2a2c30")
EQUIP_HI = hexc("#3d4045")
EQUIP_LO = hexc("#1b1c1f")
VENT = hexc("#141517")
LED = hexc("#ffb43a")
DISPLAY = hexc("#121110")
INSULATOR = hexc("#c4bdad")
INSULATOR_LO = hexc("#8c8576")
CARBON = hexc("#3b3c40")
MASK = hexc("#17181b")
MASK_HI = hexc("#2a2c31")
LAMP_RING = hexc("#555b63")

# ---------------------------------------------------------------- liveries

LG = "minecraft:light_gray_concrete"
WH = "minecraft:white_concrete"


def livery(body, lower, stripe, door, roof):
    return {"body": body, "lower": lower, "stripe": stripe, "door": door, "roof": roof}


SBAHNS = {
    "sb": livery("minecraft:yellow_terracotta", "minecraft:red_concrete", "minecraft:yellow_terracotta",
                 "minecraft:red_concrete", LG),
    "sd": livery(WH, LG, "minecraft:red_concrete", "minecraft:red_concrete", LG),
    "sg": livery(WH, "minecraft:green_concrete", "minecraft:green_concrete", "minecraft:green_concrete", LG),
}
REGIONALS = {
    "rr": livery("minecraft:red_concrete", LG, WH, LG, LG),
    "rb": livery(WH, "minecraft:blue_concrete", "minecraft:blue_concrete", "minecraft:blue_concrete", LG),
    "rg": livery(WH, "minecraft:lime_concrete", "minecraft:green_concrete", "minecraft:lime_concrete", LG),
    "rk": livery("minecraft:yellow_concrete", "minecraft:gray_concrete", "minecraft:gray_concrete",
                 "minecraft:gray_concrete", LG),
}
DECKS = {
    "dr": livery("minecraft:red_concrete", LG, WH, LG, LG),
    "db": livery(WH, "minecraft:blue_concrete", "minecraft:blue_concrete", "minecraft:blue_concrete", LG),
}


# ---------------------------------------------------------------- small helpers


def fl(v):
    return math.floor(v + 1e-6)


def rel(c):
    """The texel's spot from the box's own lowest corner, before any turn."""
    return [c.loc[i] - c.box.frm[i] for i in range(3)]


def size(c):
    return [c.box.to[i] - c.box.frm[i] for i in range(3)]


def outward(c):
    """Distance along z from the box's end that faces the nearer end of the carriage."""
    z0, z1 = c.box.frm[2], c.box.to[2]
    if z0 + z1 > 0:
        return z1 - c.loc[2]
    return c.loc[2] - z0


def slope_z(base, top, y):
    return base[0] + (y - base[1]) * (top[0] - base[0]) / (top[1] - base[1])


class Fam:
    """One family's dimensions, in units."""

    def __init__(self, **kw):
        self.__dict__.update(kw)


def pane_glass(u, v, w, h, shade):
    """A pane w x h, u and v from its lower near corner: tinted, lighter towards the top
    (`shade` 0..1 of the band), with two short glints of sky up high."""
    c = mix(GLASS_DARK, GLASS_MID, shade)
    iu, iv = fl(u), fl(v)
    if w >= 7 and h >= 7:
        for gu, gv, n in ((w * 0.2, h * 0.55, 4), (w * 0.2 + 3, h * 0.55, 2)):
            du, dv = iu - fl(gu), iv - fl(gv)
            if 0 <= dv < n and du == dv:
                return GLASS_HI if gu == w * 0.2 else GLASS_LIGHT
    return c


# ---------------------------------------------------------------- font

GLYPHS = {
    "S": ["###", "#..", "###", "..#", "###"],
    "1": [".#.", "##.", ".#.", ".#.", "###"],
    "2": ["##.", "..#", ".#.", "#..", "###"],
    "3": ["##.", "..#", ".#.", "..#", "##."],
    "R": ["##.", "#.#", "##.", "#.#", "#.#"],
    "E": ["###", "#..", "##.", "#..", "###"],
    "C": [".##", "#..", "#..", "#..", ".##"],
    "I": ["###", ".#.", ".#.", ".#.", "###"],
    "T": ["###", ".#.", ".#.", ".#.", ".#."],
    "Y": ["#.#", "#.#", ".#.", ".#.", ".#."],
    "H": ["#.#", "#.#", "###", "#.#", "#.#"],
    "B": ["##.", "#.#", "##.", "#.#", "##."],
    "F": ["###", "#..", "##.", "#..", "#.."],
    "O": [".#.", "#.#", "#.#", "#.#", ".#."],
    "N": ["#.#", "###", "###", "#.#", "#.#"],
    "D": ["##.", "#.#", "#.#", "#.#", "##."],
    "L": ["#..", "#..", "#..", "#..", "###"],
    "A": [".#.", "#.#", "###", "#.#", "#.#"],
    "U": ["#.#", "#.#", "#.#", "#.#", "###"],
    "M": ["#.#", "###", "###", "#.#", "#.#"],
    " ": ["..", "..", "..", "..", ".."],
}


def text_rows(s):
    rows = ["", "", "", "", ""]
    for ch in s:
        g = GLYPHS[ch]
        for k in range(5):
            rows[k] += g[k] + "."
    return [r[:-1] for r in rows]


def display_paint(text):
    """A destination display, amber dots on black, the text centred on the front face."""
    rows = text_rows(text)
    w = len(rows[0])

    def paint(c):
        if not c.front:
            return DISPLAY
        sx = size(c)[0]
        sy = size(c)[1]
        u = fl(c.box.to[0] - c.loc[0])  # reading from the left as seen from ahead
        v = fl(c.box.to[1] - c.loc[1])
        u0 = fl((sx - w) / 2)
        v0 = fl((sy - 5) / 2)
        if 0 <= v - v0 < 5 and 0 <= u - u0 < w and rows[v - v0][u - u0] == "#":
            return LED
        return DISPLAY
    return paint


def side_display_paint(text):
    """A destination display in a side window, read from outside on either side."""
    rows = text_rows(text)
    w = len(rows[0])

    def paint(c):
        if not c.side:
            return DISPLAY
        L = size(c)[2]
        u = fl(c.box.to[2] - c.loc[2]) if c.n[0] > 0 else fl(c.loc[2] - c.box.frm[2])
        v = fl(c.box.to[1] - c.loc[1])
        u0 = fl((L - w) / 2)
        v0 = fl((size(c)[1] - 5) / 2)
        if 0 <= v - v0 < 5 and 0 <= u - u0 < w and rows[v - v0][u - u0] == "#":
            return LED
        return DISPLAY
    return paint


def side_display(f, name, z0, y0, text_len=18, h=7):
    """A glowing display box standing on a window band, from z0 along +z."""
    return pair(Box(f"{name}_right", (f.P, y0, z0), (f.P + 0.25, y0 + h, z0 + text_len), "side_display",
                    lamp="glow"))


# ---------------------------------------------------------------- shared parts


def paint_bellows(c):
    x, y, z = c.p
    if c.front or c.back:
        # The far face: the gangway door seen through the open bellows.
        if abs(x) < 9 and y < c.box.to[1] - 4:
            return RUBBER_HI if fl(abs(x)) == 8 or fl(y) == fl(c.box.to[1]) - 5 else RUBBER
        return BELLOWS
    if c.side or c.top:
        return BELLOWS_HI if fl(z) % 2 == 0 else BELLOWS
    return RUBBER


def bellows(f, end):
    """The gangway bellows at the carriage end `end` (-1 or 1), out to the neighbour's."""
    hx, y0, y1 = f.bellows
    z0, z1 = sorted((end * f.E, end * (f.H + REACH)))
    return Box(f"bellows_{'f' if end < 0 else 'r'}", (-hx, y0, z0), (hx, y1, z1), "bellows")


def drawbar(f, end, y0, y1):
    """The short drawbar under an EMU's gangway, meeting the neighbour's halfway."""
    z0, z1 = sorted((end * (f.E - 8), end * (f.H + REACH)))
    return Box(f"drawbar_{'f' if end < 0 else 'r'}", (-2.5, y0, z0), (2.5, y1, z1), "drawbar")


def paint_drawbar(c):
    if c.top:
        return FRAME_HI
    if c.front or c.back:
        return STEEL
    return FRAME


def paint_wheel(c):
    x, y, z = c.p
    wz, wy, r = c.box.extra["wheel"]
    if c.side:
        d = math.hypot(z - wz, y - wy)
        if d > r:
            return FRAME_LO
        if d > r - 1.0:
            return TREAD
        if d < 1.6:
            return AXLE_CAP
        if d > r - 2.0:
            return WHEEL
        return WHEEL_DISC
    return TREAD if abs(x) > c.box.to[0] - 2.5 else FRAME_LO


def paint_frame(c):
    x, y, z = c.p
    if c.top:
        return FRAME_HI
    if c.bottom:
        return FRAME_LO
    iy = fl(y)
    if iy == fl(c.box.to[1] - 0.5):
        return FRAME_HI
    if iy == fl(c.box.frm[1]):
        return FRAME_LO
    return FRAME


def paint_axlebox(c):
    """An axle box, its round cover low down, a coil spring standing on it."""
    x, y, z = c.p
    if not c.side:
        return FRAME_HI if c.top else FRAME
    zc = (c.box.frm[2] + c.box.to[2]) / 2
    y0 = c.box.frm[1]
    dz, dy = z - zc, y - (y0 + 2.5)
    iy = fl(y - y0)
    if iy >= 5:
        # the coil spring under the frame: light turns on a dark core
        return FRAME_LO if abs(dz) > 1.5 else AXLE_CAP
    if math.hypot(dz, dy) < 1.8:
        return AXLE_CAP_HI if (fl(dz) == -1 and fl(dy) == 0) else AXLE_CAP
    return FRAME_HI if iy == 4 else FRAME


def paint_airspring(c):
    x, y, z = c.p
    if c.side or c.front or c.back:
        return RUBBER_HI if fl(y) % 3 == 1 else RUBBER
    return RUBBER


def bogie(f, name, zc, top=None):
    """A two-axle bogie centred on zc: wheelsets, a side frame dropped between the axle
    boxes, coil springs, and air springs up to the body at `top`."""
    r, a = f.wheel_r, f.axle
    top = f.floor if top is None else top
    out = []
    for k, za in enumerate((zc - a, zc + a)):
        wheel = (za, r, r)
        out.append(Box(f"{name}_wheels{k}_a", (-13, r - 4, za - r), (13, r + 4, za + r), "wheel", wheel=wheel))
        # the crown, 0.25 in from the disc's face so the two never share a plane
        out.append(Box(f"{name}_wheels{k}_b", (-12.75, 0, za - 4), (12.75, 2 * r, za + 4), "wheel", wheel=wheel))
        out += pair(Box(f"{name}_axlebox{k}_right", (13.5, r - 2.5, za - 2.5), (18, r + 3.5, za + 2.5), "axlebox"))
    out += pair(Box(f"{name}_beam_right", (14, r + 3, zc - a - 5), (17.5, r + 6, zc + a + 5), "frame"))
    out += pair(Box(f"{name}_drop_right", (14, r - 1.5, zc - a + 3.5), (17.5, r + 3, zc + a - 3.5), "frame"))
    out.append(Box(f"{name}_transom", (-14, r - 1, zc - 3), (14, r + 3, zc + 3), "frame"))
    if top - (r + 6) >= 1.5:
        out += pair(Box(f"{name}_airspring_right", (10.5, r + 6, zc - 4), (16.5, top, zc + 4), "airspring"))
    return out


def paint_equip(c):
    """Underfloor equipment boxes: a lit top edge, louvres, a dark foot."""
    x, y, z = c.p
    if c.bottom:
        return EQUIP_LO
    if c.top:
        return EQUIP
    y0, y1 = c.box.frm[1], c.box.to[1]
    iy = fl(y - y0)
    h = fl(y1 - y0)
    if iy == h - 1:
        return EQUIP_HI
    if iy == 0:
        return EQUIP_LO
    if c.side:
        u = rel(c)[2]
        L = size(c)[2]
        iu = fl(u)
        if iu == 0 or iu == fl(L) - 1:
            return EQUIP_LO
        if L / 3 <= u < 2 * L / 3 and 2 <= iy <= h - 3:
            return VENT if iy % 2 == 0 else EQUIP
    return EQUIP


def ac_unit(name, zc, base, half_len=20, hw=12, h=4.5):
    return Box(name, (-hw, base, zc - half_len), (hw, base + h, zc + half_len), "ac")


def paint_ac(c):
    """An air-conditioning unit on the roof: two fans on top, louvres round it."""
    x, y, z = c.p
    if c.top:
        L = size(c)[2]
        u = rel(c)[2]
        for fz in (L * 0.27, L * 0.73):
            dx, dz = x, u - fz
            d = math.hypot(dx, dz)
            if d < 5.2:
                if d < 1.5:
                    return EQUIP_HI
                if abs(dx) < 0.6 or abs(dz) < 0.6:
                    return EQUIP
                return VENT
            if d < 6.2:
                return Paint("roof", -0.32)
        if abs(x) > c.box.to[0] - 1:
            return Paint("roof", 0.12)
        return Paint("roof", 0.02)
    if c.bottom:
        return UNDER
    iy = fl(y - c.box.frm[1])
    if iy == fl(size(c)[1]) - 1:
        return Paint("roof", 0.08)
    if 1 <= iy <= 2:
        return Paint("roof", -0.42) if iy == 1 else Paint("roof", -0.24)
    return Paint("roof", -0.12)


def pantograph(prefix, zp, roof):
    """A single-arm pantograph standing on four insulators, its knee pointing ahead."""
    out = []
    for sx in (-1, 1):
        for sz in (-1, 1):
            out.append(Box(f"{prefix}_insulator_{'lr'[sx > 0]}{'fb'[sz > 0]}",
                           (sx * 6 - 1, roof, zp + sz * 8 - 1), (sx * 6 + 1, roof + 2.5, zp + sz * 8 + 1),
                           "insulator"))
    out += pair(Box(f"{prefix}_rail_right", (5, roof + 2.5, zp - 10), (8, roof + 3.5, zp + 10), "panto"))
    out.append(Box(f"{prefix}_cross", (-5, roof + 2.5, zp + 6), (5, roof + 3.5, zp + 9), "panto"))
    out.append(Box(f"{prefix}_drive", (-3, roof + 3.5, zp + 4), (3, roof + 5.5, zp + 9), "panto"))
    # lower arm: from the hinge at the back of the base up to the knee ahead of it
    hz, hy = zp + 6.5, roof + 4.5
    kz, ky = zp - 9, roof + 10.5
    la = math.degrees(math.atan2(ky - hy, hz - kz))
    ll = math.hypot(ky - hy, hz - kz)
    out.append(Box(f"{prefix}_lower_arm", (-1.5, hy - 0.5, hz - ll), (1.5, hy + 0.5, hz), "panto",
                   origin=(0, hy, hz), rot=(la, 0, 0)))
    # upper arm: from the knee back up to the head over the base
    tz, ty = zp + 2, roof + 16
    ua = math.degrees(math.atan2(ty - ky, tz - kz))
    ul = math.hypot(ty - ky, tz - kz)
    out.append(Box(f"{prefix}_upper_arm", (-0.5, ky - 0.5, kz), (0.5, ky + 0.5, kz + ul), "panto",
                   origin=(0, ky, kz), rot=(-ua, 0, 0)))
    out.append(Box(f"{prefix}_head", (-10, ty - 0.5, tz - 1.5), (10, ty + 0.5, tz + 1.5), "panto_head"))
    out += pair(Box(f"{prefix}_horn_right", (10, ty - 1.5, tz - 1), (12.5, ty + 0.25, tz + 1), "panto_head"))
    return out


def paint_panto(c):
    if c.top:
        return hexc("#8a9098")
    if c.bottom:
        return FRAME
    return STEEL


def paint_panto_head(c):
    if c.top:
        return CARBON if abs(c.p[0]) < 9 else STEEL
    return FRAME


def paint_insulator(c):
    if c.top or c.bottom:
        return INSULATOR_LO
    return INSULATOR if fl(c.p[1] * 2) % 2 == 0 else INSULATOR_LO


def lamp_paint(kind):
    """A headlight (white, a light ring round it) or a tail light (red), on a lamp box's
    front face."""
    def paint(c):
        if not c.front:
            return HOUSING_DARK
        u = fl(c.box.to[0] - c.loc[0])
        v = fl(c.box.to[1] - c.loc[1])
        nu, nv = fl(size(c)[0] + 0.5), fl(size(c)[1] + 0.5)
        edge = u == 0 or v == 0 or u == nu - 1 or v == nv - 1
        if kind == "head":
            if edge:
                return LAMP_RING
            return DRL if v == 1 else LENS
        return TAIL_DARK if edge else TAIL_BRIGHT
    return paint


def paint_housing(c):
    if c.front:
        if fl(c.box.to[1] - c.loc[1]) == 0:
            return MASK_HI
        return MASK
    if c.top:
        return MASK_HI
    return MASK


# ---------------------------------------------------------------- the body


def body_boxes(f, segs):
    """Body, window bands and roof for each segment (z0, z1, kind): kinds "end" (towards a
    gangway), "door", "bay" (windows between doors) and "cabbay" (windows behind the
    cab). Ends inset the roof a little, as Rust does."""
    out = []
    for i, seg in enumerate(segs):
        z0, z1, kind = seg[:3]
        floor = seg[3] if len(seg) > 3 else f.floor
        mat = {"door": "doorseg", "end": "endseg"}.get(kind, "body")
        out.append(Box(f"body_{i}_{kind}", (-f.W, floor, z0), (f.W, f.top, z1), mat))
        if kind not in ("door", "cabbay"):
            a, b = z0, z1
            if kind == "end":
                if z1 > 0:
                    b = z1 - f.end_blank
                else:
                    a = z0 + f.end_blank
            for k, (y0, y1) in enumerate(f.wins_for(kind) if hasattr(f, "wins_for") else f.wins):
                out += band(f"band_{i}_{k}", f, y0, y1, a, b, f.panes(kind, b - a, k))
        for k, (hw, y0, y1, inset) in enumerate(f.roof):
            a, b = z0, z1
            if kind == "cabbay" and f.cab_roof_inset:
                a = z0 + f.cab_roof_inset[k]
            if kind == "end":
                if z1 > 0:
                    b = z1 - inset
                else:
                    a = z0 + inset
            out.append(Box(f"roof{k}_{i}", (-hw, y0, a), (hw, y1, b), "roof"))
    return out


def band(name, f, y0, y1, a, b, panes):
    """A window band on both sides, standing proud of the body and not reaching into it,
    so riders see nothing of it from inside."""
    return pair(Box(name + "_right", (f.W, y0, a), (f.P, y1, b), "band", panes=panes))


def paint_band(c):
    """The window band standing proud of the body: black, with flush panes of glass."""
    x, y, z = c.p
    if c.top:
        return MASK_HI
    if c.bottom:
        return GLOSS
    if not c.side:
        return MASK
    y0, y1 = c.box.frm[1], c.box.to[1]
    iy = fl(y)
    u = rel(c)[2]
    if iy == fl(y0) or iy == fl(y1 - 1):
        return MASK
    for a, b in c.box.extra["panes"]:
        if a <= u < b:
            iu = fl(u - a)
            if iu == 0 or iu == fl(b - a) - 1 or iy == fl(y1 - 2):
                return GLASS_DARK
            # mirror the glints on the left side, so both sides read alike
            uu = (u - a) if c.n[0] > 0 else (b - u)
            return pane_glass(uu, y - y0 - 1, b - a, y1 - y0 - 2, (y - y0) / (y1 - y0))
    return MASK


def paint_roof(c):
    x, y, z = c.p
    if c.top:
        L = size(c)[2]
        u = rel(c)[2]
        hw = c.box.to[0]
        if abs(x) > hw - 1:
            return Paint("roof", 0.1)
        if L > 20 and fl(u - L / 2) % 24 == 0:
            return Paint("roof", -0.07)
        return Paint("roof", 0.0)
    if c.bottom:
        return UNDER
    if c.side:
        return Paint("roof", -0.1)
    return Paint("roof", -0.16)


def paint_side_common(f, c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        if abs(x) > f.W - 1:
            return Paint("body", 0.22)
        return Paint("roof", 0.04)
    return f.row(fl(y), c.box.frm[1])


def door_paint(f, du, y):
    """A double sliding door, `du` from its middle: two leaves with tall windows, a rubber
    seal between them and a dark gap round them."""
    a = abs(du)
    half = f.door_w / 2
    iy = fl(y)
    top = f.door_top
    if a >= half - 1 or iy >= top - 1:
        return GLOSS
    if a < 1:
        return RUBBER
    w0, w1 = f.door_win
    if w0 <= y < w1 and 2 <= a < half - 2:
        if fl(a) == 2 or fl(a) == fl(half - 3) or iy == fl(w0) or iy == fl(w1 - 1):
            return MASK
        uu = a - 3
        return pane_glass(uu, y - w0 - 1, half - 6, w1 - w0 - 2, (y - w0) / (w1 - w0))
    if iy == fl(top) - 2:
        return Paint("door", 0.14)
    if iy <= f.floor:
        return Paint("door", -0.35)
    if iy <= f.floor + 2:
        return Paint("door", -0.12)
    return Paint("door", 0.0)


def make_body_painters(f):
    def end_wall(c):
        x, y, z = c.p
        hx, y0, y1 = f.bellows
        ax = abs(x)
        if ax < hx + 1.5 and y0 - 1.5 < y < y1 + 1.5:
            return MASK_HI if (ax >= hx + 0.5 or y < y0 - 0.5 or y >= y1 + 0.5) else MASK
        if ax > f.W - 1:
            return Paint("body", -0.2) if fl(y) >= f.wins[0][0] else Paint("lower", -0.25)
        return f.row(fl(y), c.box.frm[1])

    def body(c):
        if c.front or c.back:
            return end_wall(c)
        return paint_side_common(f, c)

    def endseg(c):
        if c.front or c.back:
            return end_wall(c)
        if c.side and outward(c) < 1:
            return Paint("body", -0.25) if fl(c.p[1]) >= f.wins[0][0] else Paint("lower", -0.3)
        return paint_side_common(f, c)

    def doorseg(c):
        x, y, z = c.p
        if c.side:
            du = rel(c)[2] - size(c)[2] / 2
            if abs(du) < f.door_w / 2 and y < f.door_top:
                return door_paint(f, du, y)
        if c.front or c.back:
            return end_wall(c)
        return paint_side_common(f, c)

    return {"body": body, "endseg": endseg, "doorseg": doorseg}


def common_painters(f):
    p = {
        "band": paint_band,
        "roof": paint_roof,
        "bellows": paint_bellows,
        "wheel": paint_wheel,
        "frame": paint_frame,
        "axlebox": paint_axlebox,
        "airspring": paint_airspring,
        "equip": paint_equip,
        "ac": paint_ac,
        "panto": paint_panto,
        "panto_head": paint_panto_head,
        "insulator": paint_insulator,
        "head": lamp_paint("head"),
        "tail": lamp_paint("tail"),
        "housing": paint_housing,
        "drawbar": paint_drawbar,
        "trim": lambda c: MASK_HI if c.top else TRIM,
    }
    p.update(make_body_painters(f))
    return p


def door_steps(f, doors):
    return [Box(f"step_{k}", (-f.P, f.floor - 1.5, dz - f.door_w / 2 + 1), (f.P, f.floor, dz + f.door_w / 2 - 1),
                "trim") for k, dz in enumerate(doors)]


# ---------------------------------------------------------------- S-Bahn

SB = Fam(
    key="sbahn", H=147.2, E=145.0, W=24.0, P=24.5, floor=16.0, top=63.0,
    wins=((32.0, 49.0),), door_w=22.0, door_top=51.0, door_win=(29.0, 47.0),
    bogie_z=107.0, wheel_r=6.5, axle=13.5,
    roof=((22.0, 63.0, 65.5, 1.5), (16.0, 65.5, 67.5, 3.0)),
    bellows=(17.0, 18.0, 59.0), end_blank=4.0, roof_top=67.5, cab_roof_inset=None,
)
SB_DOORS = (-90.0, 0.0, 90.0)


def sb_row(iy, floor=None):
    """S-Bahn body side, by height: the lower colour up to the windows with the stripe
    under them, the body colour above, a gutter and a lit shoulder."""
    if iy <= 16:
        return Paint("lower", -0.32)
    if iy == 17:
        return Paint("lower", -0.14)
    if iy < 22:
        return Paint("lower", -0.07)
    if iy < 29:
        return Paint("lower", -0.03)
    if iy < 31:
        return Paint("stripe", 0.0)
    if iy < 32:
        return Paint("lower", 0.06)
    if iy < 49:
        return Paint("body", -0.05)
    if iy == 59:
        return Paint("body", -0.16)
    if iy == 61:
        return Paint("body", 0.1)
    if iy >= 62:
        return Paint("body", 0.18)
    return Paint("body", 0.02)


def sb_panes(kind, length, row):
    if kind == "bay":
        half = length / 2
        return [(1, half - 1), (half + 1, length - 1)]
    if kind == "cabbay":
        return [(1, 11.5), (13.5, length - 1)]
    return [(1, length - 1)]


SB.row = sb_row
SB.panes = sb_panes


def sb_segments(cab):
    segs = [(-128.0, -102.0, "cabbay")] if cab else [(-SB.E, -102.0, "end")]
    segs += [(-102.0, -78.0, "door"), (-78.0, -12.0, "bay"), (-12.0, 12.0, "door"),
             (12.0, 78.0, "bay"), (78.0, 102.0, "door"), (102.0, SB.E, "end")]
    return segs


def sb_equipment(f):
    out = []
    for s in (-1, 1):
        n = "f" if s < 0 else "r"
        out.append(Box(f"equip_a_{n}", (-19, 7, min(s * 84, s * 52)), (19, f.floor, max(s * 84, s * 52)), "equip"))
        out.append(Box(f"equip_b_{n}", (-17, 8.5, min(s * 46, s * 18)), (17, f.floor, max(s * 46, s * 18)), "equip"))
    out.append(Box("equip_c", (-15, 9.5, -12), (15, f.floor, 12), "equip"))
    return out


def sb_running_gear(f, third_rail=False):
    out = []
    for s in (-1, 1):
        n = f"bogie_{'f' if s < 0 else 'r'}"
        out += bogie(f, n, s * f.bogie_z)
        if third_rail:
            # a collector shoe hung from the bogie frame, out over the third rail
            out += pair(Box(f"{n}_shoe_arm_right", (17.5, 2.5, s * f.bogie_z - 1.5), (19.5, 7.5, s * f.bogie_z + 1.5),
                            "frame"))
            out += pair(Box(f"{n}_shoe_right", (18, 1, s * f.bogie_z - 3), (23, 2.5, s * f.bogie_z + 3), "frame"))
    return out + sb_equipment(f)


def sb_mid():
    f = SB
    boxes = body_boxes(f, sb_segments(False))
    boxes += [bellows(f, -1), bellows(f, 1), drawbar(f, -1, 10, 13), drawbar(f, 1, 10, 13)]
    boxes += door_steps(f, SB_DOORS)
    boxes += sb_running_gear(f)
    boxes += [ac_unit("ac_f", -118, f.roof_top), ac_unit("ac_r", 118, f.roof_top)]
    boxes += side_display(f, "side_display", -30, 41, 16)
    P = common_painters(f)
    P["side_display"] = side_display_paint("S1")
    return Model("train_sbahn_mid", boxes, P, SBAHNS["sb"], eye=(0.0, RIDER_EYE, 2.3 * M))


def windscreen_fillers(f, base, top, thick, step, mat, foot_mat):
    """Thin steps along the back of a raked windscreen slab, just inside its side faces:
    with the full-width steps behind the slab they cover its whole inner face and its top
    end, so a rider looking ahead sees through it; from outside they hide behind the
    slab's edge. The foot one, under the slab's base, is painted like the body below."""
    lean = math.atan2(top[0] - base[0], top[1] - base[1])
    back = thick / math.cos(lean)  # the slab's inner face, this far behind its outer one
    x0, x1 = f.W - 0.5, f.W + 0.25
    foot = base[1] - thick * math.sin(lean) - 0.5
    out = pair(Box("ws_filler_foot_right", (x0, foot, slope_z(base, top, foot) + back - 0.1),
                   (x1, base[1], slope_z(base, top, base[1]) + back + 0.5), foot_mat))
    y, k = base[1], 0
    while y < top[1] - 1e-6:
        y1 = min(y + step, top[1])
        z0 = slope_z(base, top, y) + back - 0.1
        if y1 >= top[1] - 1e-6:
            z0 = min(z0, top[0] + 0.2)  # and under the slab's top end
        out += pair(Box(f"ws_filler_{k}_right", (x0, y, z0), (x1, y1, slope_z(base, top, y1) + back + 0.5), mat))
        y, k = y1, k + 1
    return out


# The cab: a lower front under a windscreen raked back by about 20 degrees, a vertical
# brow over it with the top headlight, the destination display behind the glass.
SB_WS_BASE = (-145.0, 33.0)
SB_WS_TOP = (-136.0, 58.0)
SB_WS_LEN = math.hypot(SB_WS_TOP[0] - SB_WS_BASE[0], SB_WS_TOP[1] - SB_WS_BASE[1])
SB_CAB_BACK = -128.0


def sb_cab_painters(f, P):
    base_y = SB_WS_BASE[1]
    L = SB_WS_LEN

    def windscreen(c):
        if c.face != "north":
            return MASK
        lx, ly = c.loc[0], c.loc[1] - base_y
        ax, t = abs(lx), L - ly  # t: down from the top edge
        if ax > 21.5 or ly < 1 or t < 9:
            return MASK_HI if ax > 24 else MASK
        if ax > 20.5 or ly < 2 or t < 10:
            return GLOSS
        if fl(ax) == 9 and ly < 8:
            return GLOSS  # wipers parked upright
        u = lx + 20.5
        return pane_glass(u, ly - 2, 41, L - 12, ly / L)

    def lower_front(c):
        x, y, z = c.p
        iy = fl(y)
        if c.front:
            if iy >= 31:
                return MASK
            if abs(x) > f.W - 1:
                return Paint("lower", -0.22)
            if iy in (29, 30):
                return Paint("stripe", 0.0)
            if iy <= 17:
                return Paint("lower", -0.2 if iy == 16 else -0.1)
            return Paint("lower", 0.0)
        if c.side:
            return f.row(iy)
        if c.top:
            return MASK
        return UNDER

    def cab_upper(c):
        x, y, z = c.p
        iy = fl(y)
        if c.side:
            if y < f.wins[0][1]:
                return MASK
            return f.row(iy)
        if c.front:
            if iy <= 58:
                return MASK
            return Paint("body", 0.1 if iy >= 61 else 0.02)
        if c.top:
            if abs(x) > f.W - 1:
                return Paint("body", 0.22)
            return Paint("roof", 0.04)
        return MASK

    def bumper(c):
        x, y, z = c.p
        iy = fl(y)
        if c.top:
            return FRAME_HI
        if c.front:
            if abs(x) < 5 and iy < 14:
                return VENT  # the coupler pocket
            return FRAME_HI if iy % 2 == 1 else FRAME
        return FRAME

    def plough(c):
        x, y, z = c.p
        if c.front:
            iy = fl(y - c.box.frm[1])
            if iy == 0:
                return STEEL
            return FRAME if iy > 1 else FRAME_LO
        return FRAME_LO

    def coupler(c):
        x, y, z = c.p
        if c.front:
            if abs(x) < 1 and 10 <= y < 12:
                return VENT
            return STEEL if abs(x) < 2.5 and 9 <= y < 13 else FRAME
        return FRAME

    P.update({
        "windscreen": windscreen, "lower_front": lower_front, "cab_upper": cab_upper,
        "bumper": bumper, "plough": plough, "coupler": coupler, "display": display_paint("S1 CITY"),
    })


def sb_cab_boxes(f):
    out = []
    bz, by = SB_WS_BASE
    out.append(Box("cab_lower", (-f.W, f.floor, bz), (f.W, by, SB_CAB_BACK), "lower_front"))
    front, ws, _ = raked_front(f, SB_WS_BASE, SB_WS_TOP, 2.0, 5.0, SB_CAB_BACK, "cab_upper", "lower_front")
    out += front
    out.append(Box("cab_brow", (-f.W, SB_WS_TOP[1], SB_WS_TOP[0]), (f.W, f.top, SB_CAB_BACK), "cab_upper"))
    out.append(Box("cab_roof0", (-22, 63, -133), (22, 65.5, SB_CAB_BACK), "roof"))
    out.append(Box("cab_roof1", (-16, 65.5, -130), (16, 67.5, SB_CAB_BACK), "roof"))
    out.append(Box("display", (-14, by + SB_WS_LEN - 8, bz - 0.3), (14, by + SB_WS_LEN - 1, bz), "display",
                   lamp="glow", **ws))
    out.append(Box("top_light", (-2.5, 59, SB_WS_TOP[0] - 0.5), (2.5, 61.5, SB_WS_TOP[0]), "head", lamp="head"))
    out += pair(Box("lamp_housing_right", (12.5, 19.5, bz - 0.5), (23.5, 26.5, bz), "housing"))
    out += pair(Box("headlight_right", (17, 21, bz - 0.75), (22.5, 25, bz - 0.5), "head", lamp="head"))
    out += pair(Box("taillight_right", (13.5, 21, bz - 0.75), (16, 25, bz - 0.5), "tail", lamp="glow"))
    out.append(Box("bumper", (-22, 11, bz - 1), (22, f.floor, bz + 6), "bumper"))
    out.append(Box("plough", (-19, 3.5, bz - 0.5), (19, 11, bz + 5), "plough"))
    out.append(Box("coupler", (-3.5, 8.5, -f.H), (3.5, 13.5, bz - 1), "coupler"))
    out += band("band_cab", f, 32, 49, -136.5, -102, [(1, 11.5), (13.5, 33.5)])
    return out


def sb_cab(third_rail):
    f = SB
    boxes = body_boxes(f, sb_segments(True)) + sb_cab_boxes(f)
    boxes += [bellows(f, 1), drawbar(f, 1, 10, 13)]
    boxes += door_steps(f, SB_DOORS)
    boxes += sb_running_gear(f, third_rail)
    boxes += [ac_unit("ac_f", -110, f.roof_top, half_len=16), ac_unit("ac_r", 118, f.roof_top)]
    if not third_rail:
        boxes += pantograph("panto", -(f.H - 4.0 * M), f.roof_top)  # 4.0 m from the cab end
    boxes += side_display(f, "side_display", -30, 41, 16)
    P = common_painters(f)
    P["side_display"] = side_display_paint("S1")
    sb_cab_painters(f, P)
    name = "train_sbahn_cab_third_rail" if third_rail else "train_sbahn_cab"
    return Model(name, boxes, P, SBAHNS["sb"], eye=(0.0, RIDER_EYE, 2.3 * M))


# ---------------------------------------------------------------- shared cab parts


def raked_front(f, base, top, thick, step, back, mat, foot_mat):
    """A windscreen slab raked from `base` (z, y) up to `top`, the level steps behind it
    back to z=`back` (painted `mat`) and the fillers that hide its inner face."""
    lean = math.degrees(math.atan2(top[0] - base[0], top[1] - base[1]))
    length = math.hypot(top[0] - base[0], top[1] - base[1])
    out, y, k = [], base[1], 0
    while y < top[1] - 1e-6:
        y1 = min(y + step, top[1])
        out.append(Box(f"cab_step_{k}", (-f.W, y, slope_z(base, top, y1)), (f.W, y1, back), mat))
        y, k = y1, k + 1
    out += windscreen_fillers(f, base, top, thick, step, mat, foot_mat)
    ws = dict(origin=(0, base[1], base[0]), rot=(lean, 0, 0))
    out.append(Box("windscreen", (-f.P, base[1], base[0]), (f.P, base[1] + length, base[0] + thick), "windscreen",
                   **ws))
    return out, ws, length


# ---------------------------------------------------------------- regional EMU

RG = Fam(
    key="regio", H=160.0, E=158.0, W=23.0, P=23.5, floor=12.0, top=62.0,
    wins=((28.0, 46.0),), door_w=20.0, door_top=48.0, door_win=(26.0, 45.0),
    bogie_z=122.0, wheel_r=6.0, axle=12.5,
    roof=((21.0, 62.0, 64.5, 1.5), (15.0, 64.5, 66.5, 3.0)),
    bellows=(16.5, 14.0, 56.0), end_blank=4.0, roof_top=66.5, cab_roof_inset=(2.0, 6.0),
)
RG_DOORS = (-90.0, 90.0)  # Rust: +-6.8 m, over the bogies; here at the low floor beside them


def rg_row(iy, floor=None):
    """Regional body side: a narrow lower band at the foot, the stripe under the windows,
    the body colour above with its gutter and lit shoulder."""
    if iy <= 12:
        return Paint("lower", -0.32)
    if iy == 13:
        return Paint("lower", -0.12)
    if iy < 18:
        return Paint("lower", -0.03)
    if iy == 18:
        return Paint("body", -0.14)
    if iy < 24:
        return Paint("body", -0.04)
    if iy < 26:
        return Paint("stripe", 0.0)
    if iy < 28:
        return Paint("body", 0.0)
    if iy < 46:
        return Paint("body", -0.05)
    if iy == 57:
        return Paint("body", -0.16)
    if iy == 60:
        return Paint("body", 0.1)
    if iy >= 61:
        return Paint("body", 0.18)
    return Paint("body", 0.02)


def rg_panes(kind, length, row):
    if kind == "bay":
        half = length / 2
        return [(1, half - 1), (half + 1, length - 1)]
    if kind == "end":
        half = length / 2
        return [(1, half - 1), (half + 1, length - 1)]
    return [(1, length - 1)]


RG.row = rg_row
RG.panes = rg_panes


def rg_segments(cab):
    segs = [(-130.0, -101.0, "cabbay")] if cab else [(-RG.E, -101.0, "end")]
    segs += [(-101.0, -79.0, "door"), (-79.0, 0.0, "bay"), (0.0, 79.0, "bay"),
             (79.0, 101.0, "door"), (101.0, RG.E, "end")]
    return segs


def fairing(name, z0, z1, base, hw=18.0, h=4.5, ramp=8.0):
    """A roof fairing over the air conditioning and converters: a raised box with sloped
    ends, sunk into the roof at their feet."""
    top = base + h
    a = math.degrees(math.atan2(h, ramp))
    L = math.hypot(h, ramp)
    rw = hw - 0.25  # the ramps' sides stay off the main box's, where the two overlap
    return [
        Box(f"{name}_main", (-hw, base, z0), (hw, top, z1), "fairing"),
        Box(f"{name}_ramp_f", (-rw, top - 2.5, z0 - L), (rw, top, z0), "fairing_ramp",
            origin=(0, top, z0), rot=(-a, 0, 0)),
        Box(f"{name}_ramp_r", (-rw, top - 2.5, z1), (rw, top, z1 + L), "fairing_ramp",
            origin=(0, top, z1), rot=(a, 0, 0)),
    ]


def paint_fairing(c):
    x, y, z = c.p
    if c.top:
        if abs(x) > c.box.to[0] - 1:
            return Paint("roof", 0.14)
        u = rel(c)[2]
        L = size(c)[2]
        # louvred grilles let into the top, one either side of the middle
        if 4 <= abs(x) < 13 and (L * 0.15 <= u < L * 0.4 or L * 0.6 <= u < L * 0.85):
            return Paint("roof", -0.45) if fl(u) % 2 == 0 else Paint("roof", -0.2)
        return Paint("roof", 0.04)
    if c.side:
        iy = fl(y - c.box.frm[1])
        if iy == fl(size(c)[1]) - 1:
            return Paint("roof", 0.06)
        return Paint("roof", -0.12)
    if c.bottom:
        return UNDER
    return Paint("roof", -0.08)


def paint_fairing_ramp(c):
    if c.top:
        if abs(c.p[0]) > c.box.to[0] - 1:
            return Paint("roof", 0.14)
        return Paint("roof", 0.08)
    if c.side:
        return Paint("roof", -0.12)
    return Paint("roof", -0.16)


def rg_roof_gear(f, cab):
    out = []
    rt = f.roof_top
    if cab:
        out += fairing("fair_f", -114, -102, rt)
        out += pantograph("panto", -(f.H - 5.0 * M), rt)  # 5.0 m from the cab end
        out.append(Box("breaker", (-3, rt, -60), (3, rt + 3.5, -52), "panto"))
        out.append(Box("breaker_insulator", (-1, rt + 3.5, -57), (1, rt + 6, -55), "insulator"))
        out.append(Box("roof_cable", (-1, rt, -70), (1, rt + 1, -60), "trim"))
    else:
        out += fairing("fair_f", -144, -96, rt)
    out += fairing("fair_r", 96, 144, rt)
    return out


def rg_running_gear(f):
    out = []
    for s in (-1, 1):
        out += bogie(f, f"bogie_{'f' if s < 0 else 'r'}", s * f.bogie_z)
    # a few shallow boxes under the low floor
    for s in (-1, 1):
        n = "f" if s < 0 else "r"
        out.append(Box(f"tank_{n}", (-16, 6, min(s * 70, s * 40)), (16, f.floor, max(s * 70, s * 40)), "equip"))
    return out


def rg_mid():
    f = RG
    boxes = body_boxes(f, rg_segments(False))
    boxes += [bellows(f, -1), bellows(f, 1), drawbar(f, -1, 7, 10), drawbar(f, 1, 7, 10)]
    boxes += door_steps(f, RG_DOORS)
    boxes += rg_running_gear(f)
    boxes += rg_roof_gear(f, False)
    boxes += side_display(f, "side_display", -76, 37, 16)
    P = common_painters(f)
    P.update({"fairing": paint_fairing, "fairing_ramp": paint_fairing_ramp, "side_display": side_display_paint("RE1")})
    return Model("train_regio_mid", boxes, P, REGIONALS["rr"], eye=(0.0, RIDER_EYE, 2.5 * M))


# The cab: a crash nose bulging out under a long windscreen raked back 35 degrees, the
# lamps in a black visor across the nose, the coupler behind a hatch.
RG_WS_BASE = (-153.0, 26.0)
RG_WS_TOP = (-131.0, 58.0)
RG_CAB_BACK = -130.0


def rg_cab_painters(f, P, L):
    base_y = RG_WS_BASE[1]

    def windscreen(c):
        if c.face != "north":
            return MASK
        lx, ly = c.loc[0], c.loc[1] - base_y
        ax, t = abs(lx), L - ly
        if ax > 21.5 or ly < 1 or t < 9:
            return MASK_HI if ax > 23 else MASK
        if ax > 20.5 or ly < 2 or t < 10:
            return GLOSS
        if fl(ax) == 10 and ly < 9:
            return GLOSS
        return pane_glass(lx + 20.5, ly - 2, 41, L - 12, ly / L)

    def nose(c):
        x, y, z = c.p
        iy = fl(y)
        if c.front:
            if abs(x) > f.W - 1:
                return Paint("body", -0.2)
            return f.row(iy) if iy >= 24 else Paint("body", 0.0)
        if c.side:
            return f.row(iy)
        if c.top:
            return MASK
        return UNDER

    def cab_upper(c):
        x, y, z = c.p
        iy = fl(y)
        if c.side:
            if y < f.wins[0][1]:
                return MASK
            return f.row(iy)
        if c.front:
            return Paint("body", 0.12 if iy >= 60 else 0.04)
        if c.top:
            if abs(x) > f.W - 1:
                return Paint("body", 0.22)
            return Paint("roof", 0.04)
        return MASK

    def bulge(c):
        x, y, z = c.p
        iy = fl(y)
        if c.front:
            if iy < 12:
                return Paint("lower", -0.12 if iy < 9 else 0.0)
            if iy == 12:
                return Paint("lower", 0.1)
            if abs(x) > c.box.to[0] - 1:
                return Paint("body", -0.18)
            return Paint("body", 0.04 if iy >= 22 else 0.0)
        if c.top:
            if abs(x) > c.box.to[0] - 1:
                return Paint("body", 0.04)
            return Paint("body", 0.08)
        if c.side:
            if iy < 12:
                return Paint("lower", -0.15)
            return Paint("body", -0.12)
        return UNDER

    def hatch(c):
        x, y, z = c.p
        if c.front:
            u = fl(c.box.to[0] - x)
            v = fl(c.box.to[1] - y)
            if u == 0 or v == 0 or u == fl(size(c)[0]) - 1 or v == fl(size(c)[1]) - 1:
                return Paint("lower", -0.4)
            return Paint("lower", 0.06)
        if c.top:
            return Paint("lower", 0.12)
        return Paint("lower", -0.2)

    def cab_door(c):
        """The driver's door: the livery runs on across it inside a dark gap."""
        x, y, z = c.p
        if c.side:
            u = fl(rel(c)[2])
            iy = fl(y)
            if u == 0 or u == fl(size(c)[2]) - 1 or iy == fl(c.box.to[1]) - 1 or iy == fl(c.box.frm[1]):
                return GLOSS
            return f.row(iy)
        return GLOSS

    def skirt(c):
        if c.front:
            iy = fl(c.p[1] - c.box.frm[1])
            return STEEL if iy == 0 else FRAME
        return FRAME_LO

    P.update({
        "windscreen": windscreen, "nose": nose, "cab_upper": cab_upper, "bulge": bulge,
        "hatch": hatch, "skirt": skirt, "display": display_paint("RE1 HBF"), "cab_door": cab_door,
        "grab": lambda c: STEEL if not c.bottom else FRAME,
    })


def rg_cab():
    f = RG
    boxes = body_boxes(f, rg_segments(True))
    bz, by = RG_WS_BASE
    boxes.append(Box("nose_low", (-f.W, f.floor, bz), (f.W, by, RG_CAB_BACK), "nose"))
    front, ws, L = raked_front(f, RG_WS_BASE, RG_WS_TOP, 3.0, 4.0, RG_CAB_BACK, "cab_upper", "nose")
    boxes += front
    boxes.append(Box("cab_brow", (-f.W, RG_WS_TOP[1], RG_WS_TOP[0]), (f.W, f.top, RG_CAB_BACK), "cab_upper"))
    boxes.append(Box("display", (-16, by + L - 8, bz - 0.3), (16, by + L - 1, bz), "display", lamp="glow", **ws))
    boxes.append(Box("top_light", (-2.5, 59, RG_WS_TOP[0] - 0.5), (2.5, 61.5, RG_WS_TOP[0]), "head", lamp="head"))
    boxes.append(Box("bulge", (-20, 7, -158.5), (20, 24, bz), "bulge"))
    boxes.append(Box("hatch", (-7, 9, -f.H), (7, 16, -158.5), "hatch"))
    boxes.append(Box("skirt", (-18, 3, -157), (18, 7, -148), "skirt"))
    boxes.append(Box("visor", (-19.5, 16, -159), (19.5, 22, -158.5), "housing"))
    boxes += pair(Box("headlight_right", (12, 17, -159.25), (18.5, 21, -159), "head", lamp="head"))
    boxes += pair(Box("taillight_right", (8.5, 17, -159.25), (11, 21, -159), "tail", lamp="glow"))
    boxes += band("band_cab", f, 28, 46, -135, -101, [(1, 8.5), (10.5, 19.5), (21.5, 33)])
    boxes += pair(Box("cab_door_right", (f.W, 13, -125.5), (f.W + 0.25, 48, -114.5), "cab_door"))
    boxes += pair(Box("grab_right", (f.W, 16, -113.5), (f.W + 0.75, 27, -112.5), "grab"))
    boxes += [bellows(f, 1), drawbar(f, 1, 7, 10)]
    boxes += door_steps(f, RG_DOORS)
    boxes += rg_running_gear(f)
    boxes += rg_roof_gear(f, True)
    boxes += side_display(f, "side_display", -76, 37, 16)
    P = common_painters(f)
    P.update({"fairing": paint_fairing, "fairing_ramp": paint_fairing_ramp, "side_display": side_display_paint("RE1")})
    rg_cab_painters(f, P, L)
    return Model("train_regio_cab", boxes, P, REGIONALS["rr"], eye=(0.0, RIDER_EYE, 2.5 * M))


# ---------------------------------------------------------------- double-deck coach

DK = Fam(
    key="deck", H=214.4, E=212.0, W=22.5, P=23.0, floor=10.0, top=66.0,
    wins=((22.0, 35.0), (47.0, 62.0)), door_w=22.0, door_top=44.0, door_win=(24.0, 41.0),
    bogie_z=173.0, wheel_r=6.5, axle=13.5,
    roof=((21.0, 66.0, 69.5, 1.5), (18.0, 69.5, 72.5, 3.0), (12.0, 72.5, 75.0, 4.5)),
    bellows=(15.0, 24.0, 56.0), end_blank=5.0, roof_top=75.0, cab_roof_inset=(3.0, 6.0, 10.0),
)
DK_HIGH = 20.0  # the underside over the bogies; the lower deck hangs between them at 10
DK_DOORS = (-139.0, 139.0)  # Rust: +-10.8 m, over the bogies; here low, beside them


def dk_row(iy, floor=None):
    """Double-deck side: the lower colour along the foot of each part (the low middle or
    the raised ends), the stripe between the decks, a gutter and lit shoulder."""
    floor = DK.floor if floor is None else floor
    d = iy - fl(floor)
    if d <= 0:
        return Paint("lower", -0.32)
    if d == 1:
        return Paint("lower", -0.12)
    if d < 6:
        return Paint("lower", -0.03)
    if d == 6:
        return Paint("body", -0.14)
    if 39 <= iy < 42:
        return Paint("stripe", 0.0)
    if iy in (38, 42):
        return Paint("body", 0.04)
    if iy == 64:
        return Paint("body", -0.16)
    if iy >= 65:
        return Paint("body", 0.14)
    return Paint("body", 0.0)


def dk_wins(kind):
    if kind in ("end", "deckend"):
        return ((31.0, 46.0),)
    return DK.wins


def dk_panes(kind, length, row):
    if kind == "bay":
        half = length / 2
        return [(1, half - 1), (half + 1, length - 1)]
    if kind == "deckend":
        return [(3, length - 3)]
    return [(2, length / 2 - 1), (length / 2 + 1, length - 2)]


DK.row = dk_row
DK.wins_for = dk_wins
DK.panes = dk_panes


def dk_segments(cab):
    segs = [(-190.0, -151.0, "deckend", DK_HIGH)] if cab else [(-DK.E, -151.0, "end", DK_HIGH)]
    segs += [(-151.0, -127.0, "door"), (-127.0, -64.0, "bay"), (-64.0, 0.0, "bay"), (0.0, 64.0, "bay"),
             (64.0, 127.0, "bay"), (127.0, 151.0, "door"), (151.0, DK.E, "end", DK_HIGH)]
    return segs


def buffers(f, end, name, coupled=True):
    """Side buffers, a screw coupling's hook and the headstock under the end at `end`; on a
    coupled end the buffers reach out to meet the neighbour's."""
    s = end
    zf, zh = s * f.E, s * (f.H + (REACH if coupled else 0.0))
    reach = abs(zh)
    out = [Box(f"{name}_headstock", (-21, 13, min(s * (f.E - 8), zf)), (21, DK_HIGH, max(s * (f.E - 8), zf)),
               "headstock")]
    for sx in (-1, 1):
        n = f"{name}_buffer_{'lr'[sx > 0]}"
        out.append(Box(n + "_stock", (sx * 14 - 2.5, 14, min(zf, s * (reach - 1))), (sx * 14 + 2.5, 19, max(zf, s * (reach - 1))),
                       "buffer"))
        out.append(Box(n + "_head", (sx * 14 - 3.5, 13, min(s * (reach - 1), zh)), (sx * 14 + 3.5, 20, max(s * (reach - 1), zh)),
                       "buffer_head"))
    out.append(Box(f"{name}_hook", (-1.5, 15, min(zf, s * (reach - 0.5))), (1.5, 18, max(zf, s * (reach - 0.5))), "buffer"))
    return out


def paint_headstock(c):
    x, y, z = c.p
    if c.front or c.back:
        iy = fl(y - c.box.frm[1])
        if iy == fl(size(c)[1]) - 1:
            return FRAME_HI
        return FRAME_LO if iy == 0 else FRAME
    if c.top:
        return FRAME_HI
    return FRAME


def paint_buffer(c):
    return FRAME_HI if c.top else FRAME_LO


def paint_buffer_head(c):
    """A buffer's round head: polished steel, its face lit at the top."""
    if c.front or c.back:
        x, y = c.p[0], c.p[1]
        dx, dy = x - math.copysign(14, x), y - 16.5
        d = math.hypot(dx, dy)
        if d > 3.6:
            return FRAME
        if d > 2.8:
            return STEEL
        return AXLE_CAP_HI if dy > 0.5 else CHROME_DARK
    return STEEL


def dk_roof_vents(f):
    rt = f.roof_top
    return [Box("vent_f", (-6, rt, -150), (6, rt + 1.5, -136), "vent"), Box("vent_r", (-6, rt, 136), (6, rt + 1.5, 150), "vent")]


def paint_vent(c):
    """A low roof vent: slats across its top."""
    if c.top:
        u = fl(rel(c)[2])
        if 1 <= u < fl(size(c)[2]) - 1 and abs(c.p[0]) < c.box.to[0] - 1:
            return Paint("roof", -0.4) if u % 2 == 0 else Paint("roof", -0.1)
        return Paint("roof", 0.08)
    return Paint("roof", -0.2)


def dk_painters(f):
    P = common_painters(f)
    P.update({"headstock": paint_headstock, "buffer": paint_buffer, "buffer_head": paint_buffer_head,
              "vent": paint_vent})
    return P


def dk_running_gear(f):
    out = []
    for s in (-1, 1):
        out += bogie(f, f"bogie_{'f' if s < 0 else 'r'}", s * f.bogie_z, top=DK_HIGH)
    return out


def dk_mid():
    f = DK
    boxes = body_boxes(f, dk_segments(False))
    boxes += [bellows(f, -1), bellows(f, 1)]
    boxes += buffers(f, -1, "front") + buffers(f, 1, "rear")
    boxes += door_steps(f, DK_DOORS)
    boxes += dk_running_gear(f)
    boxes += dk_roof_vents(f)
    boxes += side_display(f, "side_display", -124, 27, 16)
    P = dk_painters(f)
    P["side_display"] = side_display_paint("RE2")
    return Model("train_deck_mid", boxes, P, DECKS["dr"], eye=(0.0, RIDER_EYE, 0.0))


# The cab car's driving end: a lower front with the lamps over the buffers, a two-pane
# windscreen raked back 16 degrees at the middle level, and the tall brow of the upper deck
# over it with the destination display and the top light.
DK_WS_BASE = (-212.0, 36.0)
DK_WS_TOP = (-206.0, 57.0)
DK_CAB_BACK = -190.0
DK_CAB_DOOR = (-201.0, -191.0)


def dk_cab_side(f, y, z):
    """The cab's side: the black of the windscreen wrapping round, the driver's door with
    its drop window, the livery round them."""
    iy = fl(y)
    d0, d1 = DK_CAB_DOOR
    if d0 <= z < d1 and DK_HIGH + 1 <= y < 57:
        iz = fl(z - d0)
        if iz == 0 or iz == fl(d1 - d0) - 1 or iy == 56 or iy == fl(DK_HIGH) + 1:
            return GLOSS
        if 38 <= y < 54 and 2 <= iz <= fl(d1 - d0) - 3:
            if iy in (38, 53) or iz in (2, fl(d1 - d0) - 3):
                return MASK
            return pane_glass(iz - 3, y - 39, d1 - d0 - 6, 14, (y - 38) / 16)
    if 36 <= y < 57 and z < d0 - 0.5:
        return MASK
    return f.row(iy, DK_HIGH)


def dk_cab_painters(f, P, L):
    base_y = DK_WS_BASE[1]

    def windscreen(c):
        if c.face != "north":
            return MASK
        lx, ly = c.loc[0], c.loc[1] - base_y
        ax, t = abs(lx), L - ly
        if ax > 20 or ly < 1 or t < 1.5:
            return MASK_HI if ax > 22.5 else MASK
        if ax < 1.0:
            return MASK  # the middle pillar
        if ax > 19 or ly < 2 or t < 2.5 or ax < 2:
            return GLOSS
        u = ax - 2 if lx > 0 else 19 - ax
        return pane_glass(u, ly - 2, 17, L - 4.5, ly / L)

    def lower(c):
        x, y, z = c.p
        iy = fl(y)
        if c.front:
            if iy >= 34:
                return MASK
            if abs(x) > f.W - 1:
                return Paint("body", -0.2) if iy > fl(DK_HIGH) + 5 else Paint("lower", -0.25)
            return f.row(iy, DK_HIGH)
        if c.side:
            return dk_cab_side(f, y, z)
        if c.top:
            return MASK
        return UNDER

    def upper(c):
        x, y, z = c.p
        iy = fl(y)
        if c.side:
            return dk_cab_side(f, y, z)
        if c.front:
            ax = abs(x)
            if 56 <= y < 63 and ax < 16:
                return MASK  # the display's black surround
            if iy == 64 or iy == 63 and ax > 16:
                return Paint("body", 0.0)
            return Paint("body", 0.12 if iy >= 65 else 0.02)
        if c.top:
            if abs(x) > f.W - 1:
                return Paint("body", 0.2)
            return Paint("roof", 0.04)
        return MASK

    P.update({
        "windscreen": windscreen, "dk_lower": lower, "dk_upper": upper,
        "display": display_paint("RE2 OST"), "grab": lambda c: STEEL if not c.bottom else FRAME,
    })


def dk_cab():
    f = DK
    boxes = body_boxes(f, dk_segments(True))
    bz, by = DK_WS_BASE
    boxes.append(Box("cab_lower", (-f.W, DK_HIGH, bz), (f.W, by, DK_CAB_BACK), "dk_lower"))
    front, ws, L = raked_front(f, DK_WS_BASE, DK_WS_TOP, 2.5, 7.0, DK_CAB_BACK, "dk_upper", "dk_lower")
    boxes += front
    boxes.append(Box("cab_brow", (-f.W, DK_WS_TOP[1], DK_WS_TOP[0]), (f.W, f.top, DK_CAB_BACK), "dk_upper"))
    for k, ((hw, y0, y1, _), z0) in enumerate(zip(f.roof, (-204.0, -201.0, -197.0))):
        boxes.append(Box(f"cab_roof{k}", (-hw, y0, z0), (hw, y1, DK_CAB_BACK), "roof"))
    tz = DK_WS_TOP[0]
    boxes.append(Box("display", (-14, 57, tz - 0.3), (14, 62, tz), "display", lamp="glow"))
    boxes.append(Box("top_light", (-2.5, 63, tz - 0.5), (2.5, 65, tz), "head", lamp="head"))
    boxes += pair(Box("lamp_housing_right", (10.5, 26, bz - 0.5), (21.5, 32, bz), "housing"))
    boxes += pair(Box("headlight_right", (15, 27, bz - 0.75), (21, 31, bz - 0.5), "head", lamp="head"))
    boxes += pair(Box("taillight_right", (11.5, 27, bz - 0.75), (14, 31, bz - 0.5), "tail", lamp="glow"))
    boxes += pair(Box("grab_right", (f.W, 24, -189.5), (f.W + 0.75, 50, -188.5), "grab"))
    boxes += pair(Box("cab_step_plate_right", (f.W - 4, DK_HIGH - 1.5, DK_CAB_DOOR[0] + 1),
                      (f.W + 0.5, DK_HIGH, DK_CAB_DOOR[1] - 1), "trim"))
    boxes += buffers(f, -1, "front", coupled=False) + buffers(f, 1, "rear")
    boxes += [bellows(f, 1)]
    boxes += door_steps(f, DK_DOORS)
    boxes += dk_running_gear(f)
    boxes += dk_roof_vents(f)
    boxes += side_display(f, "side_display", -124, 27, 16)
    P = dk_painters(f)
    P["side_display"] = side_display_paint("RE2")
    dk_cab_painters(f, P, L)
    return Model("train_deck_cab", boxes, P, DECKS["dr"], eye=(0.0, RIDER_EYE, 0.0))


MODELS = [sb_mid, lambda: sb_cab(False), lambda: sb_cab(True), rg_mid, rg_cab, dk_mid, dk_cab]
