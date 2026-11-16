"""Light cars: a three-box saloon, an estate, a compact crossover and a city car."""
import math

from kit import (
    AMBER, CHROME, CHROME_DARK, DRL, GLOSS, HOUSING, HOUSING_DARK, LENS, MIRROR_GLASS,
    PLATE, PLATE_BLUE, PLATE_TEXT, REVERSE, RIM, RIM_DARK, RIM_LIP, HUB, STEEL, TAIL_BRIGHT,
    TAIL_DARK, TAIL_RED, TIRE, TIRE_SIDE, TREAD_GROOVE, TREAD_RIB, TRIM, UNDER, Box, Model,
    Paint, glass, hexc, mix, pair,
)

# ---------------------------------------------------------------- wheels


def pixel_wheel(rows, colours, tyre_from=0.9):
    """A wheel face drawn as pixel art: `rows` top to bottom, one character per texel,
    centred on the wheel; texels outside the drawing (the tyre boxes' sides) are tyre,
    its side wall up to `tyre_from` of the radius."""
    n = len(rows)
    assert all(len(r) == n for r in rows)

    def face(dz, dy, radius):
        i, j = math.floor(dz + n / 2), math.floor(n / 2 - dy)
        if 0 <= i < n and 0 <= j < n:
            return colours[rows[j][i]]
        return TIRE_SIDE if math.hypot(dz, dy) < radius * tyre_from else TIRE
    return face


TYRES = {"t": TIRE, "T": TIRE_SIDE}

# Saloon: bright silver, a ring of eight slots round a small hub.
WHEEL_SALOON = pixel_wheel([
    "tTLLLLTt",
    "TLDSSDLT",
    "LDSSSSDL",
    "LSSHHSSL",
    "LSSHHSSL",
    "LDSSSSDL",
    "TLDSSDLT",
    "tTLLLLTt",
], dict(TYRES, L=RIM_LIP, S=RIM, D=RIM_DARK, H=HUB))

# Estate: graphite, four broad spokes and deep windows.
WHEEL_ESTATE = pixel_wheel([
    "tTllllTt",
    "TlDSSDlT",
    "lDDSSDDl",
    "lSSHHSSl",
    "lSSHHSSl",
    "lDDSSDDl",
    "TlDSSDlT",
    "tTllllTt",
], dict(TYRES, l=hexc("#8d939b"), S=hexc("#5f656d"), D=hexc("#17181b"), H=hexc("#3c4047")))

# Crossover: black, with machined silver spoke faces; a deeper tyre wall.
WHEEL_SUV = pixel_wheel([
    "tTTkkkTTt",
    "TkDDSDDkT",
    "kDSDSDSDk",
    "kDDSSSDDk",
    "kSSSHSSSk",
    "kDDSSSDDk",
    "kDSDSDSDk",
    "TkDDSDDkT",
    "tTTkkkTTt",
], dict(TYRES, k=hexc("#2e3136"), S=hexc("#c3c8cf"), D=hexc("#141518"), H=hexc("#55595f")))

# City car: a steel wheel under a plain plastic cover.
WHEEL_CITY = pixel_wheel([
    "tTLLLTt",
    "TLCCCLT",
    "LCdCdCL",
    "LCCHCCL",
    "LCdCdCL",
    "TLCCCLT",
    "tTLLLTt",
], dict(TYRES, L=hexc("#7d838b"), C=hexc("#c4c9cf"), d=hexc("#2a2c31"), H=hexc("#8e949c")))

# ---------------------------------------------------------------- shared parts


def slope(base, top):
    """Angle from level (degrees) and length of a glass line from base to top, (z, y)."""
    dz, dy = top[0] - base[0], top[1] - base[1]
    return math.degrees(math.atan2(dy, abs(dz))), math.hypot(dz, dy)


def z_at(base, top, y):
    """Where the glass line from base to top, (z, y), passes height y."""
    return base[0] + (y - base[1]) * (top[0] - base[0]) / (top[1] - base[1])


def windscreen_slab(name, mat, half_w, base, top, thick=1.5):
    """A slab lying under the windscreen line, its `up` face the glass."""
    ang, ln = slope(base, top)
    return Box(name, (-half_w, base[1] - thick, base[0]), (half_w, base[1], base[0] + ln), mat,
               origin=(0, base[1], base[0]), rot=(-ang, 0, 0))


def backlight_slab(name, mat, half_w, base, top, thick=1.5):
    """A sloping rear window rising forward from base (behind) to top; `up` is the glass."""
    ang, ln = slope(base, top)
    return Box(name, (-half_w, base[1] - thick, base[0] - ln), (half_w, base[1], base[0]), mat,
               origin=(0, base[1], base[0]), rot=(ang, 0, 0))


def tailgate_slab(name, mat, half_w, base, top, thick):
    """An upright rear window leaned forward from base to top; `south` is the glass.
    Thick enough to reach back into the cabin, so nothing shows between them."""
    lean = math.degrees(math.atan2(base[0] - top[0], top[1] - base[1]))
    ln = math.hypot(base[0] - top[0], top[1] - base[1])
    return Box(name, (-half_w, base[1], base[0] - thick), (half_w, base[1] + ln, base[0]), mat,
               origin=(0, base[1], base[0]), rot=(-lean, 0, 0))


def windscreen_pillar(name, mat, x0, x1, base, top, below=1.5, above=0.3):
    """The A-pillar on the right: a strip turned with the windscreen, from x0 (the
    glass's edge) out to x1, standing `above` over the glass. It covers the cabin steps'
    edges, so neither they nor the glass's underside show from inside."""
    ang, ln = slope(base, top)
    return Box(name, (x0, base[1] - below, base[0]), (x1, base[1] + above, base[0] + ln), mat,
               origin=(0, base[1], base[0]), rot=(-ang, 0, 0))


def backlight_pillar(name, mat, x0, x1, base, top, below=1.5, above=0.3):
    """The C-pillar's edge along a sloping rear window, as windscreen_pillar."""
    ang, ln = slope(base, top)
    return Box(name, (x0, base[1] - below, base[0] - ln), (x1, base[1] + above, base[0]), mat,
               origin=(0, base[1], base[0]), rot=(ang, 0, 0))


def steps(prefix, mat, half_w, ks, line, end, ahead=True):
    """Level steps that fill the cabin up to a glass line: one unit tall each, from
    the line at the step's top to `end` (behind the line if `ahead`, else in front)."""
    out = []
    for k in ks:
        zl = line(k + 1)
        z0, z1 = (zl, end) if ahead else (end, zl)
        if z1 - z0 > 0.2:
            out.append(Box(f"{prefix}_{k}", (-half_w, k, z0), (half_w, k + 1, z1), mat))
    return out


def wheels(cx, cy, cz_front, cz_rear, x_in, x_face, radius, sizes, mat="wheel"):
    """Four road wheels, each in its own bone: a tread, a crown and a face box, the
    tread and crown 0.3 behind the face, their inner sides a quarter unit apart, so no
    two of them share a plane. sizes: half (z, y) of tread, crown and face."""
    boxes, bones = [], {}
    for zname, wz in (("front", cz_front), ("rear", cz_rear)):
        bone = f"wheel_{zname}_right"
        centre = (cx, cy, wz)
        bones[bone] = centre
        bones[bone.replace("right", "left")] = (-cx, cy, wz)
        (tz, ty), (kz, ky), (fz, fy) = sizes
        for part, frm, to in (
            ("tread", (x_in, cy - ty, wz - tz), (x_face - 0.3, cy + ty, wz + tz)),
            ("crown", (x_in + 0.25, cy - ky, wz - kz), (x_face - 0.3, cy + ky, wz + kz)),
            ("face", (x_in + 0.5, cy - fy, wz - fz), (x_face, cy + fy, wz + fz)),
        ):
            boxes += pair(Box(f"{bone}_{part}", frm, to, mat, bone, wheel=centre))
    return boxes, bones


def wheel_painter(face, radius, grooves):
    def paint(c):
        wx, wy, wz = c.box.extra["wheel"]
        if c.side:
            if (c.n[0] > 0) != (wx > 0):
                return TIRE_SIDE
            return face(c.z - wz, c.y - wy, radius)
        return TREAD_GROOVE if math.floor(abs(c.x)) in grooves else TREAD_RIB
    return paint


PLATE_INK = [[0, 1, 0, 1, 1, 0, 1, 0], [0, 1, 0, 0, 1, 0, 1, 0]]


def plate(rear):
    """An EU plate, 9 x 2: the blue band on the left, a few grey characters."""
    def paint(c):
        x, y, z = c.p
        ix = round(x)
        if c.back != rear or not (c.front or c.back):
            return PLATE
        col = (4 - ix) if not rear else (ix + 4)
        if col <= 0:
            return PLATE_BLUE
        return PLATE_TEXT if PLATE_INK[0 if y > c.box.frm[1] + 1 else 1][min(col, 8) - 1] else PLATE
    return paint


def body(k):
    return Paint("body", k)


def trim_painter(c):
    return UNDER if c.bottom else TRIM


def under_painter(c):
    return UNDER


COWL = hexc("#2c2d31")
SLAT = hexc("#3a3d43")


def arch_open(z, y, arches, top=11, corner=9):
    """Whether (z, y) on a car's side lies in a wheel arch's opening; the arch's top
    corners are filled one unit in from each end."""
    for lo, hi in arches:
        if lo < z < hi and y < top:
            if y >= corner and (z < lo + 1 or z > hi - 1):
                return False
            return True
    return False


def side_rows(iy, belt):
    """The livery's shading down a car's flank: a light shoulder under the belt, a
    crease at 12 with its shadow under it, darker towards the sill."""
    k = {3: -0.24, 4: -0.14, 5: -0.06, 11: -0.12, 12: 0.12, 13: 0.07}.get(iy, 0.0)
    if 6 <= iy <= 10:
        k = -0.02 * (10 - iy) / 4
    if iy == belt - 1:
        k = 0.2
    elif iy == belt - 2 and iy > 13:
        k = 0.1
    return k


# ---------------------------------------------------------------- saloon

SD_HL = 36.8
SD_W = 14.5
SD_CAB = 13.0
SD_WS = ((-19.0, 14.0), (-5.0, 23.0))
SD_RW = ((25.0, 16.0), (14.0, 23.0))
SD_ARCHES = ((-28, -15), (15, 28))
SD_R, SD_WZ = 5.12, 21.6


def sd_arch(z, y):
    return arch_open(z, y, SD_ARCHES)


def sd_side(x, y, z):
    iy, iz = math.floor(y), math.floor(z)
    belt = 15 if z < 15 else 16
    k = side_rows(iy, belt)
    if 4 <= iy <= 14 and iz in (-14, 0):
        return body(-0.5)
    if 11 <= iy <= 14 and iz == 14:
        return body(-0.5)
    if iy == 4 and -14 <= iz < 14:
        return body(-0.38)
    if iz in (-6, -5, 8, 9):
        if iy == 12:
            return CHROME
        if iy == 11:
            return body(-0.4)
    if x > 0 and iz in (30, 31) and iy in (12, 13):
        return body(-0.16 if iy == 13 else -0.28)
    if sd_arch(z, y - 1) or sd_arch(z - 1, y) or sd_arch(z + 1, y):
        return body(min(k, 0.0) - 0.32)
    return body(k)


def sd_front(x, y):
    col, iy = math.floor(abs(x)), math.floor(y)
    if iy == 11:
        return body(0.12)
    if iy == 10:
        return CHROME if col <= 7 else body(0.0)
    if 7 <= iy <= 9 and col <= (6 if iy > 7 else 5):
        return SLAT if iy == 8 else GLOSS
    if iy >= 7:
        return body(0.0)
    if iy == 6:
        return body(0.08)
    if iy == 3:
        return TRIM
    if iy == 4:
        return GLOSS if col <= 9 else (CHROME_DARK if col == 11 else TRIM)
    if iy == 5 and col == 12:
        return TRIM
    return body(-0.06 if iy == 5 else 0.0)


def sd_rear(x, y, z):
    col, iy = math.floor(abs(x)), math.floor(y)
    if z > 36.7:
        if iy == 8:
            return body(0.08)
        if iy == 3:
            return TRIM
        if iy == 4:
            return CHROME_DARK if col in (9, 10) else GLOSS
        if iy == 6 and col in (11, 12):
            return TAIL_DARK
        return body(-0.02)
    if iy == 15:
        return body(0.14)
    if iy == 13 and col <= 8:
        return CHROME
    if iy == 12 and col <= 8:
        return body(-0.12)
    return body(0.02 if iy == 14 else -0.04)


def sd_body(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        ws_z = SD_WS[0][0]
        if ws_z - 1.5 < z < ws_z + 1 and y < 14.1:
            return COWL if abs(x) < 12.5 else body(-0.2)
        if z < ws_z and math.floor(abs(x)) == 8:
            return body(0.14)
        if y > 15.9 and z > 24:
            if math.floor(abs(x)) == 13 and z > 25:
                return body(-0.3)
            if math.floor(z) == 35:
                return body(0.16)
            return body(0.06)
        return body(0.06 if y < 14.5 else 0.0)
    if c.side:
        return sd_side(x, y, z)
    if c.front:
        if z < -36.7:
            return sd_front(x, y)
        if y < 11.5 and z > -29:
            return UNDER
        return body(0.1)
    if c.back:
        if z > 35.9:
            return sd_rear(x, y, z)
        if y < 11.5 and z < 29:
            return UNDER
        return body(-0.05)
    return body(0.0)


def sd_cabin(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    if not c.side:
        return TRIM
    zc = 12 + (23 - y)
    if z >= zc or (z > 14.5 and iy <= 16):
        return body(0.04)
    if iy == 15 or iy == 22:
        return CHROME
    zl = z_at(*SD_WS, y)
    if z - zl < 1.8 or (z < -13 and iy <= 16):
        return GLOSS
    if iz in (-1, 0, 10):
        return GLOSS
    return glass(y, z, y, top=22, bottom=15.5)


def sd_windscreen(c):
    x, y, z = c.p
    if c.face == "up":
        s = c.loc[2] - SD_WS[0][0]
        if s < 1.2 or s > slope(*SD_WS)[1] - 0.8 or abs(x) > 11.7:
            return TRIM
        return glass(y, x, s, top=22, bottom=15)
    return GLOSS


def sd_backlight(c):
    x, y, z = c.p
    if c.face == "up":
        s = SD_RW[0][0] - c.loc[2]
        ln = slope(*SD_RW)[1]
        if s < 0.8 or abs(x) > 11.7:
            return TRIM
        if s > ln - 1.0:
            return TAIL_BRIGHT if abs(x) < 3 else TRIM
        return glass(y, -x, s, top=23, bottom=16)
    if c.side:
        return body(0.04)
    return TRIM


def sd_a_pillar(c):
    if c.face == "up" or (c.side and c.n[0] * c.x > 0):
        return GLOSS
    return TRIM


def sd_c_pillar(c):
    if c.face == "up":
        return body(0.1)
    if c.side and c.n[0] * c.x > 0:
        return body(0.04)
    return TRIM


def sd_roof(c):
    x, y, z = c.p
    if c.top:
        return body(0.16 if abs(x) > 12.25 else 0.06)
    if c.bottom:
        return GLOSS
    if c.side:
        return body(-0.04)
    return body(0.0)


def sd_mirror(c):
    x, y, z = c.p
    if c.back:
        return MIRROR_GLASS
    if c.bottom:
        return GLOSS
    if c.side and abs(x) > 16.4 and math.floor(y) == 16 and z < -11.5:
        return AMBER
    if c.top:
        return body(0.12)
    return body(0.02)


def sd_headlight(c):
    x, y, z = c.p
    col, iy = math.floor(abs(x)), math.floor(y)
    if c.front:
        if iy == 10:
            return DRL
        return {8: HOUSING_DARK, 9: LENS, 10: HOUSING, 11: LENS, 12: AMBER, 13: AMBER}.get(col, HOUSING)
    if c.side:
        return DRL if iy == 10 else AMBER
    return HOUSING


def sd_taillight(c):
    x, y, z = c.p
    col, iy = round(abs(x)), math.floor(y)
    if c.back:
        if iy == 14 or col == 14:
            return TAIL_BRIGHT
        if iy == 12:
            return AMBER if col in (12, 13) else (REVERSE if col == 9 else TAIL_DARK)
        return TAIL_RED
    if c.side:
        return TAIL_BRIGHT if iy == 14 else (TAIL_RED if iy == 13 else TAIL_DARK)
    return TAIL_DARK


def build_sedan():
    hl, w = SD_HL, SD_W
    boxes = []
    add = boxes.append
    add(Box("nose", (-14, 3, -hl), (14, 12, -34), "body"))
    add(Box("front_fenders", (-w, 3, -34), (w, 13, -28), "body"))
    add(Box("front_arch_top", (-w, 11, -28), (w, 14, -15), "body"))
    add(Box("front_arch_fill_front", (-w, 9, -28), (w, 11, -27), "body"))
    add(Box("front_arch_fill_rear", (-w, 9, -16), (w, 11, -15), "body"))
    add(Box("doors", (-w, 3, -15), (w, 15, 15), "body"))
    add(Box("rear_arch_top", (-w, 11, 15), (w, 16, 28), "body"))
    add(Box("rear_arch_fill_front", (-w, 9, 15), (w, 11, 16), "body"))
    add(Box("rear_arch_fill_rear", (-w, 9, 27), (w, 11, 28), "body"))
    add(Box("rear_quarters", (-w, 3, 28), (w, 16, 36), "body"))
    add(Box("rear_bumper", (-14, 3, 36), (14, 9, hl), "body"))
    add(Box("sills", (-14, 2, -15), (14, 3, 15), "trim"))
    add(Box("underbody", (-9.5, 2.5, -33.5), (9.5, 11, 33.5), "under"))

    # Glasshouse: the cabin, level steps out to the windscreen and the rear window, the
    # two glass slabs half a unit narrower than the steps, and pillar strips turned with
    # them along their edges.
    cab = SD_CAB
    add(Box("cabin", (-cab, 15, SD_WS[1][0]), (cab, 23, SD_RW[1][0]), "cabin"))
    boxes += steps("cabin_step", "cabin", cab, range(14, 23), lambda y: z_at(*SD_WS, y), SD_WS[1][0])
    add(Box("cabin_rear_15", (-cab, 15, SD_RW[1][0]), (cab, 16, 15), "cabin"))
    boxes += steps("cabin_rear", "cabin", cab, range(16, 23), lambda y: z_at(*SD_RW, y), SD_RW[1][0], ahead=False)
    add(windscreen_slab("windscreen", "windscreen", cab - 0.5, *SD_WS))
    add(backlight_slab("rear_window", "rear_window", cab - 0.5, *SD_RW))
    boxes += pair(windscreen_pillar("a_pillar_right", "a_pillar", cab - 0.75, cab + 0.25, *SD_WS))
    boxes += pair(backlight_pillar("c_pillar_right", "c_pillar", cab - 0.75, cab + 0.25, *SD_RW))
    add(Box("roof", (-13.5, 23, SD_WS[1][0] - 0.5), (13.5, 23.5, SD_RW[1][0] + 0.5), "roof"))
    add(Box("antenna", (-0.5, 23.5, 10), (0.5, 24.25, 13.5), "trim"))
    add(Box("antenna_top", (-0.5, 24.25, 11.5), (0.5, 24.75, 13.5), "trim"))

    boxes += pair(Box("mirror_right", (w, 16, -13), (16.5, 18, -10.5), "mirror"))
    boxes += pair(Box("mirror_arm_right", (cab, 15, -12.5), (w, 16.5, -11), "trim"))

    boxes += pair(Box("headlight_right", (8, 9, -hl - 0.25), (14, 11, -hl), "headlight", lamp="head"))
    boxes += pair(Box("headlight_side_right", (14, 9, -hl), (w, 11, -34), "headlight", lamp="head"))
    boxes += pair(Box("taillight_right", (8.5, 12, 36), (w, 15, 36.25), "taillight", lamp="brake"))
    boxes += pair(Box("taillight_side_right", (w, 12, 32.5), (w + 0.25, 15, 36.25), "taillight", lamp="brake"))
    add(Box("plate_front", (-4.5, 5, -hl - 0.25), (4.5, 7, -hl), "plate_front"))
    add(Box("plate_rear", (-4.5, 10, 36), (4.5, 12, 36.25), "plate_rear"))

    wb, bones = wheels(11.6, SD_R, -SD_WZ, SD_WZ, 9.6, 13.6, SD_R, ((5, 3), (3, SD_R), (4, 4)))
    boxes += wb

    painters = {
        "body": sd_body, "trim": trim_painter, "under": under_painter, "cabin": sd_cabin,
        "windscreen": sd_windscreen, "rear_window": sd_backlight, "roof": sd_roof,
        "a_pillar": sd_a_pillar, "c_pillar": sd_c_pillar,
        "mirror": sd_mirror, "headlight": sd_headlight, "taillight": sd_taillight,
        "plate_front": plate(False), "plate_rear": plate(True),
        "wheel": wheel_painter(WHEEL_SALOON, SD_R, (10, 12)),
    }
    # Seat from cars.rs Kind::Sedan: 0.4 m ahead of the middle, eyes 1.15 m up.
    return Model(
        "car_sedan", boxes, painters, {"body": "minecraft:blue_concrete"},
        bones=bones, eye=(0.0, 1.15 * 16, -0.4 * 16),
    )


# ---------------------------------------------------------------- estate

ES_HL = 37.6
ES_W = 14.5
ES_CAB = 13.0
ES_WS = ((-20.5, 14.0), (-6.5, 22.0))
ES_TG = ((36.5, 15.0), (34.0, 22.0))  # tailgate glass, base to top
ES_ARCHES = ((-30, -17), (17, 30))
ES_R, ES_WZ = 5.12, 23.2
RAIL = hexc("#b9bfc6")
RAIL_DARK = hexc("#7f858d")


def es_arch(z, y):
    return arch_open(z, y, ES_ARCHES)


def es_d_line(y):
    """The D-pillar's front edge, leaning with the tailgate."""
    return 31.5 - (y - 15) * 2.5 / 7


def es_side(x, y, z):
    iy, iz = math.floor(y), math.floor(z)
    k = side_rows(iy, 15)
    if 4 <= iy <= 14 and iz in (-16, 0):
        return body(-0.5)
    if 11 <= iy <= 14 and iz == 16:
        return body(-0.5)
    if iy == 4 and -16 <= iz < 16:
        return body(-0.38)
    if iz in (-7, -6, 9, 10):
        if iy == 12:
            return CHROME
        if iy == 11:
            return body(-0.4)
    if x > 0 and iz in (32, 33) and iy in (12, 13):
        return body(-0.16 if iy == 13 else -0.28)
    if es_arch(z, y - 1) or es_arch(z - 1, y) or es_arch(z + 1, y):
        return body(min(k, 0.0) - 0.32)
    return body(k)


def es_front(x, y):
    col, iy = math.floor(abs(x)), math.floor(y)
    if col <= 5 and 7 <= iy <= 11:
        if iy == 11:
            return CHROME
        if col == 5:
            return CHROME_DARK
        return SLAT if col % 2 == 0 else GLOSS
    if iy == 11:
        return body(0.12)
    if iy >= 7:
        return body(0.0)
    if iy == 6:
        return body(0.08)
    if iy == 3:
        return TRIM
    if iy in (4, 5):
        if col <= 10:
            return GLOSS if iy == 4 else hexc("#2b2c31")
        return DRL if (iy == 5 and col == 12) else TRIM
    return body(-0.06)


def es_rear(x, y, z):
    col, iy = math.floor(abs(x)), math.floor(y)
    if z > 37.5:
        if iy == 8:
            return body(0.08)
        if iy <= 4:
            return GLOSS if iy == 4 and col <= 9 else TRIM
        if iy == 6 and col in (11, 12):
            return TAIL_DARK
        return body(-0.02)
    if iy == 14:
        return body(0.14)
    if iy == 13 and col <= 3:
        return CHROME
    if col == 11 and iy >= 9:
        return body(-0.35)
    return body(-0.04 if iy <= 11 else 0.02)


def es_body(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        ws_z = ES_WS[0][0]
        if ws_z - 1.5 < z < ws_z + 1 and y < 14.1:
            return COWL if abs(x) < 12.5 else body(-0.2)
        if z < ws_z and math.floor(abs(x)) == 8:
            return body(0.14)
        return body(0.06 if y < 14.5 else 0.0)
    if c.side:
        return es_side(x, y, z)
    if c.front:
        if z < -37.5:
            return es_front(x, y)
        if y < 11.5 and z > -31:
            return UNDER
        return body(0.1)
    if c.back:
        if z > 36.9:
            return es_rear(x, y, z)
        if y < 11.5 and z < 31:
            return UNDER
        return body(-0.05)
    return body(0.0)


def es_cabin(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    if not c.side:
        return TRIM
    zd = es_d_line(y)
    if z >= zd:
        return body(0.14 if iy == 15 else 0.04)
    if iy == 15:
        return GLOSS
    if iy == 21:
        return TRIM
    zl = z_at(*ES_WS, y)
    if z - zl < 1.8 or (z < -14 and iy <= 16):
        return GLOSS
    if iz in (-1, 0, 16, 17):
        return GLOSS
    return glass(y, z, y, top=21, bottom=15.5)


def es_windscreen(c):
    x, y, z = c.p
    if c.face == "up":
        s = c.loc[2] - ES_WS[0][0]
        if s < 1.2 or s > slope(*ES_WS)[1] - 0.8 or abs(x) > 11.7:
            return TRIM
        return glass(y, x, s, top=21, bottom=15)
    return GLOSS


def es_tailgate(c):
    x, y, z = c.p
    if c.face == "south":
        v = c.loc[1] - ES_TG[0][1]
        if v < 1 or v > 6.6 or abs(x) > 11.7:
            return TRIM
        if 1.5 <= v < 4.5 and abs(x - (v - 1.5) * 1.4) < 0.6:
            return hexc("#2b2c30")
        return glass(y, -x, v, top=22, bottom=16)
    if c.side:
        return body(0.04)
    return TRIM


def es_roof(c):
    x, y, z = c.p
    if c.top:
        return body(0.16 if abs(x) > 12.25 else 0.06)
    if c.bottom:
        return GLOSS
    return body(-0.04)


def es_spoiler(c):
    x, y, z = c.p
    if c.back and abs(x) <= 3.01 and y > 22.4:
        return TAIL_BRIGHT
    if c.top:
        return body(0.06)
    if c.bottom:
        return GLOSS
    return body(-0.08)


def es_rail(c):
    x, y, z = c.p
    if c.top:
        return RAIL
    if c.front or c.back:
        return TRIM
    return RAIL_DARK


def es_mirror(c):
    x, y, z = c.p
    if c.back:
        return MIRROR_GLASS
    if c.bottom:
        return GLOSS
    if c.side and abs(x) > 16.4 and math.floor(y) == 16 and z < -13.5:
        return AMBER
    return CHROME if c.top else mix(CHROME, CHROME_DARK, 0.5)


def es_headlight(c):
    """A T of light: a bar along the top, a stem down the middle."""
    x, y, z = c.p
    col, iy = math.floor(abs(x)), math.floor(y)
    if c.front:
        if iy == 11:
            return DRL if 9 <= col <= 12 else HOUSING
        if col == 10 or col == 11:
            return DRL if iy == 10 else HOUSING_DARK
        if iy == 10:
            return LENS if col in (8, 9, 12) else HOUSING
        return AMBER if col == 13 else HOUSING_DARK
    if c.side:
        return {11: DRL, 10: HOUSING, 9: AMBER}.get(iy, HOUSING)
    return HOUSING


def es_taillight(c):
    """Tall lamps up the tailgate's sides: a bright outer edge, the indicator and
    reversing light low down."""
    x, y, z = c.p
    col, iy = math.floor(abs(x)), math.floor(y)
    if c.back:
        if iy in (9, 10):
            return REVERSE if iy == 10 and col == 12 else (AMBER if iy == 9 else TAIL_DARK)
        if col == 13 or iy >= 17:
            return TAIL_BRIGHT
        return TAIL_RED if iy % 3 else TAIL_DARK
    if c.side:
        return TAIL_BRIGHT if iy >= 11 else TAIL_DARK
    return TAIL_DARK


def build_estate():
    hl, w = ES_HL, ES_W
    boxes = []
    add = boxes.append
    add(Box("nose", (-14, 3, -hl), (14, 12, -35), "body"))
    add(Box("front_fenders", (-w, 3, -35), (w, 13, -30), "body"))
    add(Box("front_arch_top", (-w, 11, -30), (w, 14, -17), "body"))
    add(Box("front_arch_fill_front", (-w, 9, -30), (w, 11, -29), "body"))
    add(Box("front_arch_fill_rear", (-w, 9, -18), (w, 11, -17), "body"))
    add(Box("doors", (-w, 3, -17), (w, 15, 17), "body"))
    add(Box("rear_arch_top", (-w, 11, 17), (w, 15, 30), "body"))
    add(Box("rear_arch_fill_front", (-w, 9, 17), (w, 11, 18), "body"))
    add(Box("rear_arch_fill_rear", (-w, 9, 29), (w, 11, 30), "body"))
    add(Box("rear_quarters", (-w, 3, 30), (w, 15, 37), "body"))
    add(Box("rear_bumper", (-14, 3, 37), (14, 9, hl), "body"))
    add(Box("sills", (-14, 2, -17), (14, 3, 17), "trim"))
    add(Box("underbody", (-9.5, 2.5, -34.5), (9.5, 11, 34.5), "under"))

    cab = ES_CAB
    add(Box("cabin", (-cab, 15, ES_WS[1][0]), (cab, 22, ES_TG[1][0]), "cabin"))
    boxes += steps("cabin_step", "cabin", cab, range(14, 22), lambda y: z_at(*ES_WS, y), ES_WS[1][0])
    add(windscreen_slab("windscreen", "windscreen", cab - 0.5, *ES_WS))
    boxes += pair(windscreen_pillar("a_pillar_right", "a_pillar", cab - 0.75, cab + 0.25, *ES_WS))
    add(tailgate_slab("tailgate_glass", "tailgate", cab - 0.5, *ES_TG, thick=4))
    add(Box("roof", (-13.5, 22, ES_WS[1][0] - 0.5), (13.5, 23, ES_TG[1][0]), "roof"))
    add(Box("spoiler", (-12.5, 22, ES_TG[1][0]), (12.5, 23, 35.5), "spoiler"))
    # Roof rails standing on three feet each, a gap of sky under them.
    boxes += pair(Box("rail_right", (11, 23.5, -4), (12.5, 24, 32), "rail"))
    for k, z in enumerate((-4, 13, 30)):
        boxes += pair(Box(f"rail_foot_{k}_right", (11, 23, z), (12.5, 23.5, z + 2), "trim"))

    boxes += pair(Box("mirror_right", (w, 16, -15.5), (16.5, 18, -13), "mirror"))
    boxes += pair(Box("mirror_arm_right", (cab, 15, -15), (w, 16.5, -13.5), "trim"))

    boxes += pair(Box("headlight_right", (8, 9, -hl - 0.25), (14, 12, -hl), "headlight", lamp="head"))
    boxes += pair(Box("headlight_side_right", (14, 9, -hl), (w, 12, -35), "headlight", lamp="head"))
    boxes += pair(Box("taillight_right", (11.5, 9, 37), (w, 15, 37.25), "taillight", lamp="brake"))
    boxes += pair(Box("taillight_high_right", (12.5, 15, 35.75), (w, 19, 37.25), "taillight", lamp="brake"))
    boxes += pair(Box("taillight_side_right", (w, 9, 34.5), (w + 0.25, 15, 37.25), "taillight", lamp="brake"))
    add(Box("plate_front", (-4.5, 5, -hl - 0.25), (4.5, 7, -hl), "plate_front"))
    add(Box("plate_rear", (-4.5, 10, 37), (4.5, 12, 37.25), "plate_rear"))

    wb, bones = wheels(11.6, ES_R, -ES_WZ, ES_WZ, 9.6, 13.6, ES_R, ((5, 3), (3, ES_R), (4, 4)))
    boxes += wb

    painters = {
        "body": es_body, "trim": trim_painter, "under": under_painter, "cabin": es_cabin,
        "windscreen": es_windscreen, "a_pillar": sd_a_pillar, "tailgate": es_tailgate,
        "roof": es_roof, "spoiler": es_spoiler, "rail": es_rail, "mirror": es_mirror,
        "headlight": es_headlight, "taillight": es_taillight,
        "plate_front": plate(False), "plate_rear": plate(True),
        "wheel": wheel_painter(WHEEL_ESTATE, ES_R, (10, 12)),
    }
    # Seat: 0.3 m ahead of the middle, eyes 1.15 m up.
    return Model(
        "car_estate", boxes, painters, {"body": "minecraft:green_concrete"},
        bones=bones, eye=(0.0, 1.15 * 16, -0.3 * 16),
    )


# ---------------------------------------------------------------- crossover

SU_HL = 36.0
SU_W = 14.5
SU_CLAD = 15.0
SU_CAB = 13.5
SU_WS = ((-18.0, 16.0), (-5.0, 25.0))
SU_TG = ((33.0, 17.0), (28.5, 25.0))
SU_ARCHES = ((-30, -15), (15, 30))
SU_R, SU_WZ = 5.76, 22.4
CLADDING = hexc("#26272b")
CLADDING_LIGHT = hexc("#323439")
SKID = hexc("#a9afb7")


def su_arch(z, y):
    return arch_open(z, y, SU_ARCHES, top=13, corner=11)


def su_d_line(y):
    return 27.0 - (y - 17) * 4.5 / 8


def su_side(x, y, z):
    iy, iz = math.floor(y), math.floor(z)
    k = side_rows(iy, 17)
    if iy in (14, 15):
        k = {14: 0.12, 15: 0.07}[iy]
    if iy == 13:
        k = -0.1
    if 5 <= iy <= 16 and iz in (-15, 0):
        return body(-0.5)
    if 13 <= iy <= 16 and iz == 14:
        return body(-0.5)
    if iz in (-7, -6, 8, 9):
        if iy == 14:
            return CHROME
        if iy == 13:
            return body(-0.4)
    if x > 0 and iz in (32, 33) and iy in (14, 15):
        return body(-0.16 if iy == 15 else -0.28)
    if iy <= 6:
        return CLADDING
    if su_arch(z, y - 1) or su_arch(z - 1, y) or su_arch(z + 1, y):
        return body(min(k, 0.0) - 0.32)
    return body(k)


def su_front(x, y):
    col, iy = math.floor(abs(x)), math.floor(y)
    width = {11: 4, 10: 5, 9: 6, 8: 7, 7: 7}.get(iy, -1)
    if col <= width:
        if iy == 11:
            return SLAT
        return SLAT if iy == 9 and col < width else GLOSS
    if iy >= 12:
        return body(0.12 if iy == 12 else 0.0)
    if iy >= 7:
        return body(0.0 if iy > 8 else -0.06)
    if iy in (4, 5) and col <= 6:
        return SKID if iy == 4 else mix(SKID, CLADDING, 0.5)
    if iy == 5 and col in (10, 11):
        return LENS if col == 11 else CLADDING
    return CLADDING_LIGHT if iy == 6 else CLADDING


def su_rear(x, y, z):
    col, iy = math.floor(abs(x)), math.floor(y)
    if z > 35.9:
        if iy == 3 or (iy == 4 and col <= 6):
            return SKID if iy == 4 else CLADDING
        if iy == 7 and col in (11, 12):
            return TAIL_DARK
        return CLADDING_LIGHT if iy == 8 else CLADDING
    if iy == 16:
        return body(0.14)
    if iy == 13 and col <= 3:
        return CHROME
    if iy == 9 and col <= 10:
        return body(-0.35)
    return body(-0.04 if iy <= 12 else 0.02)


def su_body(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        ws_z = SU_WS[0][0]
        if ws_z - 1.5 < z < ws_z + 1 and y < 16.1:
            return COWL if abs(x) < 13 else body(-0.2)
        if z < ws_z and math.floor(abs(x)) == 9:
            return body(0.14)
        if z > 35.4 and y < 9.1:
            return CLADDING_LIGHT
        return body(0.06 if y < 16.5 else 0.0)
    if c.side:
        return su_side(x, y, z)
    if c.front:
        if z < -35.9:
            return su_front(x, y)
        if y < 13.5 and z > -31:
            return UNDER
        return body(0.1)
    if c.back:
        if z > 35.4:
            return su_rear(x, y, z)
        if y < 13.5 and z < 31:
            return UNDER
        return body(-0.05)
    return body(0.0)


def su_clad(c):
    x, y, z = c.p
    if c.top:
        return CLADDING_LIGHT
    if c.bottom:
        return UNDER
    return CLADDING


def su_cabin(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    if not c.side:
        return TRIM
    if z >= su_d_line(y) or iy in (17, 24):
        return GLOSS
    zl = z_at(*SU_WS, y)
    if z - zl < 1.8 or (z < -12 and iy <= 18):
        return GLOSS
    if iz in (0, 1, 15, 16):
        return GLOSS
    return glass(y, z, y, top=24, bottom=17.5)


def su_windscreen(c):
    x, y, z = c.p
    if c.face == "up":
        s = c.loc[2] - SU_WS[0][0]
        if s < 1.2 or s > slope(*SU_WS)[1] - 0.8 or abs(x) > 12.2:
            return TRIM
        return glass(y, x, s, top=24, bottom=17)
    return GLOSS


def su_tailgate(c):
    x, y, z = c.p
    if c.face == "south":
        v = c.loc[1] - SU_TG[0][1]
        ln = math.hypot(SU_TG[0][0] - SU_TG[1][0], SU_TG[1][1] - SU_TG[0][1])
        if v < 1 or v > ln - 0.7 or abs(x) > 11.7:
            return TRIM
        if 1.5 <= v < 4.5 and abs(x - (v - 1.5) * 1.4) < 0.6:
            return hexc("#2b2c30")
        return glass(y, -x, v, top=25, bottom=18)
    return GLOSS


def su_roof(c):
    x, y, z = c.p
    if c.top:
        return body(0.16 if abs(x) > 13 else 0.06)
    if c.bottom:
        return GLOSS
    return body(-0.04)


def su_spoiler(c):
    x, y, z = c.p
    if c.back and abs(x) <= 3.01:
        return TAIL_BRIGHT
    if c.top:
        return body(0.06)
    return GLOSS


def su_rail(c):
    x, y, z = c.p
    if c.top:
        return STEEL if y > 26.9 else hexc("#3a3c42")
    return GLOSS


def su_mirror(c):
    x, y, z = c.p
    if c.back:
        return MIRROR_GLASS
    if c.side and abs(x) > 16.4 and math.floor(y) == 18 and z < -12.5:
        return AMBER
    return CLADDING_LIGHT if c.top else GLOSS


def su_drl(c):
    """A slim strip of light along the bonnet's edge over a dark housing."""
    if c.front or c.side:
        return DRL if c.y > 13 else HOUSING_DARK
    return HOUSING


def su_headlight(c):
    x, y, z = c.p
    col, iy = math.floor(abs(x)), math.floor(y)
    if c.front:
        if col in (10, 12) and iy == 9:
            return LENS
        if col == 13 and iy == 10:
            return AMBER
        return HOUSING_DARK if iy == 9 else HOUSING
    if c.side:
        return AMBER if iy == 10 else HOUSING_DARK
    return HOUSING


def su_taillight(c):
    """Full-width: a thin bar across the tailgate between two clusters."""
    x, y, z = c.p
    col, iy = math.floor(abs(x)), math.floor(y)
    if c.back:
        if col >= 9:
            if iy == 15:
                return TAIL_BRIGHT
            return AMBER if col in (10, 11) else (REVERSE if col == 9 else TAIL_RED)
        return TAIL_BRIGHT if iy == 15 else TAIL_DARK
    if c.side:
        return TAIL_BRIGHT if iy == 15 else TAIL_RED
    return TAIL_DARK


def build_suv():
    hl, w, cl = SU_HL, SU_W, SU_CLAD
    boxes = []
    add = boxes.append
    add(Box("nose", (-14, 4, -hl), (14, 14, -33), "body"))
    add(Box("front_fenders", (-w, 4, -33), (w, 15, -30), "body"))
    add(Box("front_arch_top", (-w, 13, -30), (w, 16, -15), "body"))
    add(Box("front_arch_fill_front", (-w, 11, -30), (w, 13, -29), "body"))
    add(Box("front_arch_fill_rear", (-w, 11, -16), (w, 13, -15), "body"))
    add(Box("doors", (-w, 4, -15), (w, 17, 15), "body"))
    add(Box("rear_arch_top", (-w, 13, 15), (w, 17, 30), "body"))
    add(Box("rear_arch_fill_front", (-w, 11, 15), (w, 13, 16), "body"))
    add(Box("rear_arch_fill_rear", (-w, 11, 29), (w, 13, 30), "body"))
    add(Box("rear_quarters", (-w, 4, 30), (w, 17, 35.5), "body"))
    add(Box("rear_bumper", (-14, 3, 35.5), (14, 9, hl), "body"))
    add(Box("front_skid", (-13, 3, -35.5), (13, 4, -33), "clad"))
    add(Box("underbody", (-9.5, 3, -33), (9.5, 13, 33), "under"))
    # Black plastic: a lip round each arch and the sills, standing half a unit proud.
    for name, (lo, hi) in zip(("front", "rear"), SU_ARCHES):
        boxes += pair(Box(f"clad_{name}_top_right", (w, 13, lo - 1), (cl, 14.5, hi + 1), "clad"))
        boxes += pair(Box(f"clad_{name}_fore_right", (w, 4, lo - 1), (cl, 13, lo + 1), "clad"))
        boxes += pair(Box(f"clad_{name}_aft_right", (w, 4, hi - 1), (cl, 13, hi + 1), "clad"))
    boxes += pair(Box("sill_right", (w, 3, -14), (cl, 6.5, 14), "clad"))

    cab = SU_CAB
    add(Box("cabin", (-cab, 17, SU_WS[1][0]), (cab, 25, SU_TG[1][0]), "cabin"))
    boxes += steps("cabin_step", "cabin", cab, range(16, 25), lambda y: z_at(*SU_WS, y), SU_WS[1][0])
    add(windscreen_slab("windscreen", "windscreen", cab - 0.5, *SU_WS))
    boxes += pair(windscreen_pillar("a_pillar_right", "a_pillar", cab - 0.75, cab + 0.25, *SU_WS))
    add(tailgate_slab("tailgate_glass", "tailgate", cab - 0.5, *SU_TG, thick=5))
    add(Box("roof", (-14, 25, SU_WS[1][0] - 0.5), (14, 26, SU_TG[1][0]), "roof"))
    add(Box("spoiler", (-13, 25, SU_TG[1][0]), (13, 26, 30.5), "spoiler"))
    boxes += pair(Box("rail_right", (11.5, 26.5, -2), (13, 27, 26), "rail"))
    for k, z in enumerate((-2, 11, 24)):
        boxes += pair(Box(f"rail_foot_{k}_right", (11.5, 26, z), (13, 26.5, z + 2), "rail"))

    boxes += pair(Box("mirror_right", (w, 18, -14), (16.5, 20, -11.5), "mirror"))
    boxes += pair(Box("mirror_arm_right", (cab, 17, -13.5), (w, 18.5, -12), "trim"))

    boxes += pair(Box("drl_right", (7, 12, -hl - 0.25), (14, 14, -hl), "drl", lamp="head"))
    boxes += pair(Box("drl_side_right", (14, 12, -hl), (w, 14, -33), "drl", lamp="head"))
    boxes += pair(Box("headlight_right", (9, 9, -hl - 0.25), (14, 11, -hl), "headlight", lamp="head"))
    add(Box("taillight", (-w, 14, 35.5), (w, 16, 35.75), "taillight", lamp="brake"))
    boxes += pair(Box("taillight_side_right", (w, 14, 32), (w + 0.25, 16, 35.75), "taillight", lamp="brake"))
    add(Box("plate_front", (-4.5, 6, -hl - 0.25), (4.5, 8, -hl), "plate_front"))
    add(Box("plate_rear", (-4.5, 10, 35.5), (4.5, 12, 35.75), "plate_rear"))

    wb, bones = wheels(11.75, SU_R, -SU_WZ, SU_WZ, 9.5, 14.0, SU_R, ((5.5, 3.5), (3.5, SU_R), (4.5, 4.5)))
    boxes += wb

    painters = {
        "body": su_body, "clad": su_clad, "trim": trim_painter, "under": under_painter,
        "cabin": su_cabin, "windscreen": su_windscreen, "a_pillar": sd_a_pillar,
        "tailgate": su_tailgate, "roof": su_roof, "spoiler": su_spoiler, "rail": su_rail,
        "mirror": su_mirror, "drl": su_drl, "headlight": su_headlight,
        "taillight": su_taillight, "plate_front": plate(False), "plate_rear": plate(True),
        "wheel": wheel_painter(WHEEL_SUV, SU_R, (10, 12)),
    }
    # Seat: 0.25 m ahead of the middle, eyes 1.35 m up.
    return Model(
        "car_suv", boxes, painters, {"body": "minecraft:white_concrete"},
        bones=bones, eye=(0.0, 1.35 * 16, -0.25 * 16),
    )


# ---------------------------------------------------------------- city car

CT_HL = 28.8
CT_W = 13.0
CT_CAB = 12.0
CT_WS = ((-18.5, 14.0), (-5.0, 23.0))
CT_TG = ((27.5, 14.0), (25.5, 23.0))
CT_ARCHES = ((-25, -13), (13, 25))
CT_R, CT_WZ = 4.64, 19.2
CT_ROOF = hexc("#eef0f2")
CT_ROOF_SHADE = hexc("#c9cdd2")


def ct_arch(z, y):
    return arch_open(z, y, CT_ARCHES, top=10, corner=8)


def ct_c_line(y):
    """The thick C-pillar's front edge, leaning forward towards the roof."""
    return 19.0 - (y - 14) * 0.5


def ct_side(x, y, z):
    iy, iz = math.floor(y), math.floor(z)
    k = {3: -0.24, 4: -0.14, 5: -0.06, 10: -0.12, 11: 0.12, 12: 0.07, 13: 0.2}.get(iy, 0.0)
    if 6 <= iy <= 9:
        k = -0.02 * (9 - iy) / 3
    if 4 <= iy <= 13 and iz in (-13, 7):
        return body(-0.5)
    if iy == 4 and -13 <= iz < 7:
        return body(-0.38)
    if iz in (3, 4):
        if iy == 11:
            return CHROME
        if iy == 10:
            return body(-0.4)
    if x > 0 and iz in (19, 20) and iy in (11, 12):
        return body(-0.16 if iy == 12 else -0.28)
    if iz == -15 and iy == 12:
        return AMBER
    if ct_arch(z, y - 1) or ct_arch(z - 1, y) or ct_arch(z + 1, y):
        return body(min(k, 0.0) - 0.32)
    return body(k)


def ct_front(x, y):
    """A small smiling intake under the plate, the bonnet's edge over the lamps."""
    col, iy = round(abs(x)), math.floor(y)
    if iy >= 11:
        return body(0.12)
    if iy == 3:
        return TRIM
    if iy == 6 and col <= 6:
        return GLOSS
    if iy == 7 and 6 <= col <= 7:
        return GLOSS
    if iy == 4 and col >= 10:
        return TRIM
    return body(0.04 if iy == 8 else (-0.08 if iy <= 5 else -0.03))


def ct_rear(x, y, z):
    col, iy = round(abs(x)), math.floor(y)
    if z > 28.7:
        if iy == 7:
            return body(0.08)
        if iy == 3:
            return TRIM
        if iy == 4:
            return GLOSS if col <= 8 else TRIM
        if iy == 5 and col in (10, 11):
            return TAIL_DARK
        return body(-0.02)
    if iy == 13:
        return body(0.14)
    if iy == 12 and col <= 2:
        return CHROME
    return body(-0.04 if iy <= 10 else 0.02)


def ct_body(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        ws_z = CT_WS[0][0]
        if ws_z - 1.5 < z < ws_z + 1 and y < 14.1:
            return COWL if abs(x) < 11.5 else body(-0.2)
        return body(0.06 if y < 13.5 else 0.0)
    if c.side:
        return ct_side(x, y, z)
    if c.front:
        if z < -28.7:
            return ct_front(x, y)
        if y < 10.5 and z > -26:
            return UNDER
        return body(0.1)
    if c.back:
        if z > 27.9:
            return ct_rear(x, y, z)
        if y < 10.5 and z < 26:
            return UNDER
        return body(-0.05)
    return body(0.0)


def ct_cabin(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    if not c.side:
        return TRIM
    if z >= ct_c_line(y):
        return body(0.14 if iy == 14 else 0.04)
    if iy in (14, 22):
        return TRIM
    zl = z_at(*CT_WS, y)
    if z - zl < 1.8 or (z < -12 and iy <= 15):
        return GLOSS
    if iz in (6, 7):
        return GLOSS
    return glass(y, z, y, top=22, bottom=14.5)


def ct_windscreen(c):
    x, y, z = c.p
    if c.face == "up":
        s = c.loc[2] - CT_WS[0][0]
        if s < 1.2 or s > slope(*CT_WS)[1] - 0.8 or abs(x) > 10.7:
            return TRIM
        return glass(y, x, s, top=22, bottom=14)
    return GLOSS


def ct_tailgate(c):
    x, y, z = c.p
    if c.face == "south":
        v = c.loc[1] - CT_TG[0][1]
        if v < 1 or v > 8.3 or abs(x) > 10.7:
            return TRIM
        if 1.5 <= v < 4.5 and abs(x - (v - 1.5) * 1.4) < 0.6:
            return hexc("#2b2c30")
        return glass(y, -x, v, top=23, bottom=15)
    if c.side:
        return body(0.04)
    return TRIM


def ct_roof(c):
    x, y, z = c.p
    if c.top:
        return CT_ROOF
    if c.bottom:
        return GLOSS
    return CT_ROOF_SHADE


def ct_spoiler(c):
    x, y, z = c.p
    if c.back and abs(x) <= 2.01 and y < 23.5:
        return TAIL_BRIGHT
    if c.top:
        return CT_ROOF
    if c.bottom:
        return GLOSS
    return CT_ROOF_SHADE


def ct_mirror(c):
    x, y, z = c.p
    if c.back:
        return MIRROR_GLASS
    if c.bottom:
        return GLOSS
    return CT_ROOF if c.top else CT_ROOF_SHADE


def ct_headlight(c):
    """Round lamps: a ring of light round a lens."""
    x, y, z = c.p
    col, iy = round(abs(x)), math.floor(y)
    if c.front:
        if col in (10, 11) and iy in (9, 10):
            return LENS if (col + iy) % 2 else hexc("#e3e9ef")
        return DRL
    if c.side:
        return AMBER if iy == 9 else DRL
    return HOUSING


def ct_taillight(c):
    x, y, z = c.p
    col, iy = round(abs(x)), math.floor(y)
    if c.back:
        if iy == 9:
            return AMBER if col == 12 else REVERSE
        if col == 12 or iy == 12:
            return TAIL_BRIGHT
        return TAIL_RED if iy == 11 else TAIL_DARK
    if c.side:
        return TAIL_BRIGHT if iy >= 11 else TAIL_DARK
    return TAIL_DARK


def build_city():
    hl, w = CT_HL, CT_W
    boxes = []
    add = boxes.append
    add(Box("nose", (-12.5, 3, -hl), (12.5, 12, -26.5), "body"))
    add(Box("front_fenders", (-w, 3, -26.5), (w, 13, -25), "body"))
    add(Box("front_arch_top", (-w, 10, -25), (w, 14, -13), "body"))
    add(Box("front_arch_fill_front", (-w, 8, -25), (w, 10, -24), "body"))
    add(Box("front_arch_fill_rear", (-w, 8, -14), (w, 10, -13), "body"))
    add(Box("doors", (-w, 3, -13), (w, 14, 13), "body"))
    add(Box("rear_arch_top", (-w, 10, 13), (w, 14, 25), "body"))
    add(Box("rear_arch_fill_front", (-w, 8, 13), (w, 10, 14), "body"))
    add(Box("rear_arch_fill_rear", (-w, 8, 24), (w, 10, 25), "body"))
    add(Box("rear_quarters", (-w, 3, 25), (w, 14, 28), "body"))
    add(Box("rear_bumper", (-12.5, 3, 28), (12.5, 8, hl), "body"))
    add(Box("sills", (-12.5, 2, -13), (12.5, 3, 13), "trim"))
    add(Box("underbody", (-8.5, 2.5, -26), (8.5, 10, 26), "under"))

    cab = CT_CAB
    add(Box("cabin", (-cab, 14, CT_WS[1][0]), (cab, 23, CT_TG[1][0]), "cabin"))
    boxes += steps("cabin_step", "cabin", cab, range(13, 23), lambda y: z_at(*CT_WS, y), CT_WS[1][0])
    add(windscreen_slab("windscreen", "windscreen", cab - 0.5, *CT_WS))
    boxes += pair(windscreen_pillar("a_pillar_right", "a_pillar", cab - 0.75, cab + 0.25, *CT_WS))
    add(tailgate_slab("tailgate_glass", "tailgate", cab - 0.5, *CT_TG, thick=3))
    add(Box("roof", (-12.5, 23, CT_WS[1][0] - 0.5), (12.5, 24, CT_TG[1][0]), "roof"))
    add(Box("spoiler", (-11.5, 23, CT_TG[1][0]), (11.5, 24, 27), "spoiler"))

    boxes += pair(Box("mirror_right", (w, 15, -13), (15, 17, -10.5), "mirror"))
    boxes += pair(Box("mirror_arm_right", (cab, 14, -12.5), (w, 15.5, -11), "trim"))

    # Round lamps: a plus of three boxes, so the corners stay body colour.
    for part, frm, to in (
        ("", (9.5, 8, -hl - 0.25), (11.5, 12, -hl)),
        ("_in", (8.5, 9, -hl - 0.25), (9.5, 11, -hl)),
        ("_out", (11.5, 9, -hl - 0.25), (12.5, 11, -hl)),
    ):
        boxes += pair(Box(f"headlight{part}_right", frm, to, "headlight", lamp="head"))
    boxes += pair(Box("headlight_side_right", (12.5, 9, -hl), (w, 11, -26.5), "headlight", lamp="head"))
    boxes += pair(Box("taillight_right", (9.5, 9, 28), (w, 13, 28.25), "taillight", lamp="brake"))
    boxes += pair(Box("taillight_side_right", (w, 10, 26), (w + 0.25, 13, 28.25), "taillight", lamp="brake"))
    add(Box("plate_front", (-4.5, 4, -hl - 0.25), (4.5, 6, -hl), "plate_front"))
    add(Box("plate_rear", (-4.5, 9, 28), (4.5, 11, 28.25), "plate_rear"))

    wb, bones = wheels(10.45, CT_R, -CT_WZ, CT_WZ, 8.7, 12.2, CT_R, ((4.5, 2.5), (2.5, CT_R), (3.5, 3.5)))
    boxes += wb

    painters = {
        "body": ct_body, "trim": trim_painter, "under": under_painter, "cabin": ct_cabin,
        "windscreen": ct_windscreen, "a_pillar": sd_a_pillar, "tailgate": ct_tailgate,
        "roof": ct_roof, "spoiler": ct_spoiler, "mirror": ct_mirror,
        "headlight": ct_headlight, "taillight": ct_taillight,
        "plate_front": plate(False), "plate_rear": plate(True),
        "wheel": wheel_painter(WHEEL_CITY, CT_R, (9, 11)),
    }
    # Seat: 0.05 m ahead of the middle, eyes 1.2 m up.
    return Model(
        "car_city", boxes, painters, {"body": "minecraft:cyan_concrete"},
        bones=bones, eye=(0.0, 1.2 * 16, -0.05 * 16),
    )


MODELS = [build_sedan, build_estate, build_suv, build_city]
