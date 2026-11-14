"""A five-door hatchback, 4.0 x 1.76 x 1.5 m, two-tone with a black roof."""
import math

from kit import (
    AMBER, CHROME, CHROME_DARK, DRL, GLOSS, HOUSING, HOUSING_DARK, LENS, MIRROR_GLASS, PLATE,
    PLATE_BLUE, PLATE_TEXT, REVERSE, TAIL_BRIGHT, TAIL_DARK, TREAD_GROOVE, TREAD_RIB,
    TIRE_SIDE, TRIM, UNDER, Box, Model, Paint, glass, hexc, mix, pair, wheel_face,
    GLASS_DARK, GLASS_MID,
)

ROOF = hexc("#1f2024")
ROOF_EDGE = hexc("#34363c")
FOG = hexc("#e9ecef")

# Windscreen: from the scuttle at (z, y) = (-15, 14) up to the roof at (-1.5, 23), 3:2.
WS_BASE = (-15.0, 14.0)
WS_TOP = (-1.5, 23.0)
WS_ANGLE = math.degrees(math.atan2(WS_TOP[1] - WS_BASE[1], WS_TOP[0] - WS_BASE[0]))
WS_LEN = math.hypot(WS_TOP[1] - WS_BASE[1], WS_TOP[0] - WS_BASE[0])
# Rear window: from the tailgate at (30.5, 15) up to the roof at (24.5, 23), 3:4. It is an
# upright slab leaned forward, since Java turns a box by 45 degrees at most.
RW_BASE = (30.5, 15.0)
RW_TOP = (24.5, 23.0)
RW_LEAN = math.degrees(math.atan2(RW_BASE[0] - RW_TOP[0], RW_TOP[1] - RW_BASE[1]))
RW_LEN = math.hypot(RW_TOP[1] - RW_BASE[1], RW_BASE[0] - RW_TOP[0])

WHEEL_Z, WHEEL_Y, WHEEL_X = 20.0, 5.0, 11.0


def arch_open(z, y):
    for lo, hi in ((-26, -14), (14, 26)):
        if lo < z < hi and y < 11:
            if y >= 9 and (z < lo + 1 or z > hi - 1):
                return False
            return True
    return False


def body_side(x, y, z):
    iy, iz = math.floor(y), math.floor(z)
    k = {14: 0.20, 13: 0.07, 12: 0.12, 11: -0.12, 5: -0.06, 4: -0.14, 3: -0.24}.get(iy, 0.0)
    if 6 <= iy <= 10:
        k = -0.02 * (10 - iy) / 4
    if 4 <= iy <= 14 and iz in (-14, 7):
        return Paint("body", -0.5)
    if 11 <= iy <= 14 and iz == 19:
        return Paint("body", -0.5)
    if iy == 4 and -14 <= iz < 19 and not arch_open(z, y - 1):
        return Paint("body", -0.38)
    if iz in (1, 2, 13, 14):
        if iy == 12:
            return CHROME
        if iy == 11:
            return Paint("body", -0.4)
    if x > 0 and iz in (22, 23) and iy in (11, 12):
        return Paint("body", -0.16 if iy == 12 else -0.28)
    if iz == -16 and iy == 13:
        return AMBER
    if arch_open(z, y - 1) or arch_open(z - 1, y) or arch_open(z + 1, y):
        return Paint("body", min(k, 0.0) - 0.32)
    return Paint("body", k)


def front_face(x, y):
    ax, ix, iy = abs(x), round(x), math.floor(y)
    if iy >= 12:
        return Paint("body", 0.12)
    if iy >= 9:
        if ax <= 1.01 and iy in (9, 10):
            return CHROME_DARK if ix == 0 and iy == 9 else CHROME
        if iy == 11:
            return CHROME_DARK
        return GLOSS
    if iy >= 7:
        return Paint("body", 0.05 if iy == 8 else -0.04)
    if iy == 3:
        return TRIM
    if ax <= 8.01:
        return GLOSS if iy % 2 == 0 else hexc("#2b2c31")
    if ax <= 11.01:
        if round(ax) == 10 and iy == 5:
            return FOG
        return TRIM
    return Paint("body", -0.1)


def rear_face(x, y, z):
    ax, ix, iy = abs(x), round(x), math.floor(y)
    if z > 31.9:
        if iy == 8:
            return Paint("body", 0.08)
        if iy <= 4:
            if ix in (-9, -8):
                return CHROME_DARK if iy == 4 else hexc("#0c0c0d")
            return TRIM
        if iy == 6 and round(ax) in (11, 12):
            return TAIL_DARK
        return Paint("body", 0.0)
    if iy == 14:
        return Paint("body", 0.12)
    if iy in (10, 11) and ax <= 5.01:
        return Paint("body", -0.35)
    if iy == 13 and ax <= 1.01:
        return CHROME
    if iy == 12 and ax <= 3.01:
        return CHROME_DARK
    return Paint("body", -0.04 if iy <= 10 else 0.0)


def paint_body(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        if y > 14.9 and z < WS_BASE[0] + 1.5 and abs(x) < 12.6:
            return hexc("#2c2d31") if 2 <= abs(x) <= 10.5 else TRIM
        if z < WS_BASE[0] and y < 14.1 and math.floor(abs(x)) == 7:
            return Paint("body", 0.14)
        if y < 13.1:
            return Paint("body", 0.06)
        return Paint("body", 0.0)
    if c.side:
        return body_side(x, y, z)
    if c.front:
        if z < -31.9:
            return front_face(x, y)
        if y < 11.5 and z > -27:
            return UNDER
        return Paint("body", 0.1)
    if c.back:
        if z > 31.4:
            return rear_face(x, y, z)
        if y < 11.5 and z < 27:
            return UNDER
        return Paint("body", -0.05)
    return Paint("body", 0.0)


def paint_cabin(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    if not c.side:
        return TRIM
    pillar = 19.5 - 0.5 * (y - 16)
    if z >= pillar:
        return Paint("body", 0.14 if iy == 15 else 0.04)
    if z < -7.5 and iy <= 17:
        return GLOSS
    if iy == 15:
        return CHROME
    if iy >= 22:
        return TRIM
    if 7 <= iz <= 8:
        return GLOSS
    return glass(y, z, y)


def paint_windscreen(c):
    x, y, z = c.p
    if c.face == "up":
        if y > 22.3 or abs(x) > 11.4 or y < 15.4:
            return TRIM
        return glass(y, x, c.loc[2] - WS_BASE[0])
    return GLOSS


def paint_rear_window(c):
    x, y, z = c.p
    if c.face == "south":
        if y > 22.3 or abs(x) > 11.4 or y < 15.9:
            return TRIM
        if 16 <= y < 19.5 and abs(x + (y - 16) * 1.6) < 0.6:
            return hexc("#2b2c30")
        return glass(y, -x, c.loc[1] - RW_BASE[1])
    if c.side:
        # The window's frame along its edge, then the body-coloured D-pillar.
        if c.loc[2] > RW_BASE[0] - 0.9:
            return TRIM
        return Paint("body", 0.04)
    return TRIM


def paint_roof(c):
    x, y, z = c.p
    if c.top:
        if abs(x) > 11.75:
            return ROOF_EDGE
        if abs(x) <= 7.01 and 1 <= z < 13:
            if abs(x) > 6 or z < 2 or z >= 12:
                return GLOSS
            return glass(18, x, z)
        return ROOF
    if c.bottom:
        return GLOSS
    return mix(ROOF, (0, 0, 0), 0.1)


def paint_spoiler(c):
    x, y, z = c.p
    if c.back and abs(x) <= 3.01:
        return TAIL_BRIGHT
    if c.top:
        return ROOF
    return mix(ROOF, (0, 0, 0), 0.15)


def paint_mirror(c):
    x, y, z = c.p
    if c.back:
        return MIRROR_GLASS
    if c.side and abs(x) > 15.9 and math.floor(y) == 15:
        return AMBER
    return ROOF if c.top else mix(ROOF, (0, 0, 0), 0.1)


def paint_headlight(c):
    x, y, z = c.p
    ax, iy = round(abs(x)), math.floor(y)
    if c.front:
        if iy == 11:
            return DRL
        if iy == 10:
            if ax == 13:
                return DRL
            return LENS if ax in (9, 11) else HOUSING
        return AMBER if ax in (12, 13) else HOUSING_DARK
    if c.side:
        return {11: DRL, 10: HOUSING, 9: AMBER}.get(iy, HOUSING)
    return HOUSING


def paint_taillight(c):
    x, y, z = c.p
    ax, iy = round(abs(x)), math.floor(y)
    if c.back:
        if iy == 13:
            return TAIL_DARK if ax < 13 else hexc("#a3141f")
        if iy == 12:
            return TAIL_BRIGHT
        if ax in (8, 9):
            return REVERSE
        if ax in (12, 13):
            return AMBER
        return TAIL_DARK
    if c.side:
        return TAIL_BRIGHT if iy == 12 else TAIL_DARK
    return TAIL_DARK


PLATE_INK = [[0, 1, 0, 1, 1, 0, 1, 0], [0, 1, 0, 0, 1, 0, 1, 0]]


def plate(rear):
    def paint(c):
        x, y, z = c.p
        ix = round(x)
        if c.back != rear or not (c.front or c.back):
            return PLATE
        col = (4 - ix) if not rear else (ix + 4)
        if col == 0:
            return PLATE_BLUE
        return PLATE_TEXT if PLATE_INK[0 if y > c.box.frm[1] + 1 else 1][col - 1] else PLATE
    return paint


def paint_wheel(c):
    x, y, z = c.p
    wx, wy, wz = c.box.extra["wheel"]
    if c.side:
        if (c.n[0] > 0) != (wx > 0):
            return TIRE_SIDE
        return wheel_face(z - wz, y - wy)
    return TREAD_GROOVE if math.floor(abs(x)) in (10, 12) else TREAD_RIB


def build():
    boxes = []
    add = boxes.append
    # Lower body, front to back; the wheel arches are the gaps between these.
    add(Box("nose", (-13.5, 3, -32), (13.5, 13, -29), "body"))
    add(Box("front_fenders", (-14, 3, -29), (14, 14, -26), "body"))
    add(Box("front_arch_top", (-14, 11, -26), (14, 14, -14), "body"))
    add(Box("front_arch_fill_front", (-14, 9, -26), (14, 11, -25), "body"))
    add(Box("front_arch_fill_rear", (-14, 9, -15), (14, 11, -14), "body"))
    add(Box("doors", (-14, 3, -14), (14, 15, 14), "body"))
    add(Box("rear_arch_top", (-14, 11, 14), (14, 15, 26), "body"))
    add(Box("rear_arch_fill_front", (-14, 9, 14), (14, 11, 15), "body"))
    add(Box("rear_arch_fill_rear", (-14, 9, 25), (14, 11, 26), "body"))
    add(Box("rear_quarters", (-14, 3, 26), (14, 15, 28.5), "body"))
    add(Box("tailgate", (-13.5, 3, 28.5), (13.5, 15, 31.5), "body"))
    add(Box("rear_bumper", (-13.5, 3, 31.5), (13.5, 9, 32), "body"))
    add(Box("sills", (-13.5, 2, -14), (13.5, 3, 14), "trim"))
    add(Box("underbody", (-9, 2.25, -29), (9, 11, 29), "under"))

    # Glasshouse: the cabin, level steps under the windscreen that carry the door glass
    # forward to the A-pillar, the windscreen slab (narrower than the steps, so its
    # underside stays inside them and out of the rider's view), A-pillar strips over its
    # edges and the rear window slab.
    add(Box("cabin", (-12.5, 15, -1.5), (12.5, 23, 24.5), "cabin"))
    for k in range(15, 22):
        z0 = WS_BASE[0] + (k + 1 - WS_BASE[1]) * (WS_TOP[0] - WS_BASE[0]) / (WS_TOP[1] - WS_BASE[1])
        add(Box(f"cabin_step_{k}", (-12.5, k, z0), (12.5, k + 1, -1.5), "cabin"))
    add(Box(
        "windscreen", (-12, WS_BASE[1] - 1.5, WS_BASE[0]), (12, WS_BASE[1], WS_BASE[0] + WS_LEN),
        "windscreen", origin=(0, WS_BASE[1], WS_BASE[0]), rot=(-WS_ANGLE, 0, 0),
    ))
    boxes += pair(Box(
        "a_pillar_right", (11.75, WS_BASE[1] - 1.5, WS_BASE[0]), (13, WS_BASE[1] + 0.3, WS_BASE[0] + WS_LEN),
        "a_pillar", origin=(0, WS_BASE[1], WS_BASE[0]), rot=(-WS_ANGLE, 0, 0),
    ))
    add(Box(
        "rear_window", (-12.25, RW_BASE[1], RW_BASE[0] - 5), (12.25, RW_BASE[1] + RW_LEN, RW_BASE[0]),
        "rear_window", origin=(0, RW_BASE[1], RW_BASE[0]), rot=(-RW_LEAN, 0, 0),
    ))
    add(Box("roof", (-12.75, 23, -2), (12.75, 24, 25), "roof"))
    add(Box("spoiler", (-12, 23.5, 23.5), (12, 24.5, 27), "spoiler"))
    add(Box("antenna", (-0.5, 24, 18.5), (0.5, 24.5, 22), "roof"))
    add(Box("antenna_tip", (-0.5, 24.5, 19.5), (0.5, 25, 22), "roof"))

    boxes += pair(Box("mirror_right", (14, 15.5, -9), (16, 17.5, -6.5), "mirror"))
    boxes += pair(Box("mirror_arm_right", (12.5, 15, -8.5), (14, 16, -7.5), "trim"))

    boxes += pair(Box("headlight_right", (6.5, 9, -32.25), (13.5, 12, -32), "headlight", lamp="head"))
    boxes += pair(Box("headlight_side_right", (13.5, 9, -32), (13.75, 12, -29.5), "headlight", lamp="head"))
    boxes += pair(Box("taillight_right", (7.5, 11, 31.5), (13.5, 14, 31.75), "taillight", lamp="brake"))
    boxes += pair(Box("taillight_side_right", (13.5, 11, 28.5), (13.75, 14, 31.5), "taillight", lamp="brake"))
    add(Box("plate_front", (-4.5, 6, -32.25), (4.5, 8, -32), "plate_front"))
    add(Box("plate_rear", (-4.5, 10, 31.5), (4.5, 12, 31.75), "plate_rear"))

    bones = {}
    for zname, wz in (("front", -WHEEL_Z), ("rear", WHEEL_Z)):
        bone = f"wheel_{zname}_right"
        centre = (WHEEL_X, WHEEL_Y, wz)
        bones[bone] = centre
        bones[bone.replace("right", "left")] = (-WHEEL_X, WHEEL_Y, wz)
        for part, frm, to in (
            ("tread", (9.0, 2, wz - 5), (12.75, 8, wz + 5)),
            ("crown", (9.25, 0, wz - 3), (12.75, 10, wz + 3)),
            ("face", (9.5, 1, wz - 4), (13.0, 9, wz + 4)),
        ):
            boxes += pair(Box(f"{bone}_{part}", frm, to, "wheel", bone, wheel=centre))

    painters = {
        "body": paint_body,
        "trim": lambda c: UNDER if c.bottom else TRIM,
        "under": lambda c: UNDER,
        "cabin": paint_cabin,
        "windscreen": paint_windscreen,
        "a_pillar": lambda c: GLOSS if c.face == "up" or (c.side and c.n[0] * c.p[0] > 0) else TRIM,
        "rear_window": paint_rear_window,
        "roof": paint_roof,
        "spoiler": paint_spoiler,
        "mirror": paint_mirror,
        "headlight": paint_headlight,
        "taillight": paint_taillight,
        "plate_front": plate(False),
        "plate_rear": plate(True),
        "wheel": paint_wheel,
    }
    # Seat from cars.rs Kind::Hatchback: 0.1 m ahead of the middle, eyes 1.2 m up.
    return Model(
        "car_hatchback", boxes, painters, {"body": "minecraft:red_concrete"},
        bones=bones, eye=(0.0, 1.2 * 16, -0.1 * 16),
    )


MODELS = [build]
