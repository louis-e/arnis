"""The heavier road vehicles: a panel van, a double-cab pickup, a rigid box truck and a
low-floor city bus. Lengths, widths, axles and wheel sizes from cars.rs `parts()`, the
rider's eye from `Kind::seat`."""
import math

from kit import (
    AMBER, CHROME, CHROME_DARK, DRL, GLASS_DARK, GLASS_HI, GLASS_LIGHT, GLASS_MID, GLOSS,
    HOUSING, HOUSING_DARK, HUB, LENS, MIRROR_GLASS, PLATE, PLATE_BLUE, PLATE_TEXT, REVERSE,
    RIM, RIM_DARK, RIM_LIP, STEEL, TAIL_BRIGHT, TAIL_DARK, TIRE, TIRE_SIDE, TREAD_GROOVE,
    TREAD_RIB, TRIM, UNDER, Box, Model, Paint, glass, hexc, mix, pair,
)

M = 16.0  # units to a metre

PLASTIC = hexc("#2b2c31")
PLASTIC_HI = hexc("#3a3c42")
PLASTIC_LO = hexc("#202124")
FOG = hexc("#e9ecef")
TAIL_MID = hexc("#a3141f")


# ---------------------------------------------------------------- shared helpers


def xs(c):
    """x of a texel counted from the middle of its face, for a face running across a box
    centred on x = 0: whole numbers when the face is an odd number of texels wide, so
    features painted from it come out symmetric even on a face a fraction of a texel
    wider than a whole number."""
    size = c.box.to[0] - c.box.frm[0]
    n = math.floor(size + 1e-6)
    v = c.i - (n - 1) / 2
    return -v if c.face == "north" else v


def ax(c):
    return abs(xs(c))


def slab(name, base, top, half_w, thick, mat, **kw):
    """A flat panel from `base` (z, y) to `top` (z, y), turned about x, `thick` units deep
    under its upper face; for windscreens and bonnets sloping up rearwards. `half_w` is a
    half width or an (x0, x1) span."""
    (z0, y0), (z1, y1) = base, top
    angle = math.degrees(math.atan2(y1 - y0, z1 - z0))
    length = math.hypot(y1 - y0, z1 - z0)
    if isinstance(half_w, tuple):
        x0, x1 = half_w
    else:
        x0, x1 = -half_w, half_w
    return Box(name, (x0, y0 - thick, z0), (x1, y0, z0 + length), mat,
               origin=(0, y0, z0), rot=(-angle, 0, 0), **kw)


def upright(name, base, top, half_w, thick, mat, **kw):
    """An upright panel from `base` (z, y) up to `top` (z, y), leaning by turning about
    x, `thick` units deep behind (+z) its front face; for nearly upright windscreens."""
    (z0, y0), (z1, y1) = base, top
    lean = math.degrees(math.atan2(z1 - z0, y1 - y0))
    length = math.hypot(y1 - y0, z1 - z0)
    if isinstance(half_w, tuple):
        x0, x1 = half_w
    else:
        x0, x1 = -half_w, half_w
    return Box(name, (x0, y0, z0), (x1, y0 + length, z0 + thick), mat,
               origin=(0, y0, z0), rot=(lean, 0, 0), **kw)


def corner(name, at, cut, y0, y1, mat, depth=2.0, rear=False, **kw):
    """A 45-degree chamfer on a corner of the right side: its face runs from `at` (x, z),
    the edge of the front (or rear) face, to the side `cut` units further out, filling
    `depth` units in behind."""
    x0, z0 = at
    diag = cut * math.sqrt(2)
    if rear:
        return Box(name, (x0, y0, z0 - depth), (x0 + diag, y1, z0), mat,
                   origin=(x0, y0, z0), rot=(0, 45, 0), **kw)
    return Box(name, (x0, y0, z0), (x0 + diag, y1, z0 + depth), mat,
               origin=(x0, y0, z0), rot=(0, -45, 0), **kw)


def wheel_boxes(bones, name, centre, x_in, x_out, steps, mat="wheel"):
    """A road wheel on the right as boxes stacked into a disc, and its mirror image, each
    in its own bone. `steps` are (half length, half height, inset) from the tread round
    to the crown, the inset how far a box's outer face stands in from `x_out`: boxes that
    overlap stand a quarter unit apart, or behind a third, so no two faces flicker."""
    wx, wy, wz = centre
    bone = f"wheel_{name}_right"
    bones[bone] = (wx, wy, wz)
    bones[bone.replace("right", "left")] = (-wx, wy, wz)
    out = []
    for k, (hz, hy, inset) in enumerate(steps):
        out += pair(Box(f"{bone}_{k}", (x_in + 0.25 * k, wy - hy, wz - hz), (x_out - inset, wy + hy, wz + hz),
                        mat, bone, wheel=(wx, wy, wz)))
    return out


def plate(rear):
    """A European plate: blue band on the left, a few grey marks for letters."""
    ink = [[0, 1, 0, 1, 1, 0, 1, 0], [0, 1, 0, 0, 1, 0, 1, 0]]

    def paint(c):
        if not (c.front if not rear else c.back):
            return PLATE
        col = c.i
        if col == 0:
            return PLATE_BLUE
        row = 0 if c.j == 0 else 1
        if 1 <= col <= 8 and ink[row][col - 1]:
            return PLATE_TEXT
        return PLATE
    return paint


def mirror_painter(edge_x):
    """A wing mirror: glass facing back, an indicator strip along its outer side."""
    def paint(c):
        if c.back:
            return MIRROR_GLASS
        if c.side and abs(c.x) > edge_x - 0.1 and math.floor(c.y) == math.floor(c.box.to[1] - 1.5):
            return AMBER
        if c.top:
            return PLASTIC_HI
        return PLASTIC if not c.bottom else PLASTIC_LO
    return paint


# ---------------------------------------------------------------- van

V_HL = 39.2            # half length, 4.9 m
V_HW = 15.6            # half width, 1.95 m
V_CAB = 14.6           # glasshouse and load box, a unit in from the sides
V_ROOF = 13.6
V_BELT = 19
V_TOP = 31.2           # 1.95 m
V_WS_BASE = (-28.0, 18.5)
V_WS_TOP = (-15.0, 30.0)
V_BONNET = (-38.0, 16.0)
V_WZ, V_WR = 25.5, 5.5  # axles 0.85 m from the ends, wheels 0.68 m


def v_arch_open(z, y):
    for wz in (-V_WZ, V_WZ):
        lo, hi = wz - 6.5, wz + 6.5
        if lo < z < hi and y < 12:
            if y >= 10 and (z < lo + 1 or z > hi - 1):
                return False
            return True
    return False


def v_arch_edge(z, y):
    return not v_arch_open(z, y) and (v_arch_open(z, y - 1) or v_arch_open(z - 1, y) or v_arch_open(z + 1, y))


def van_side(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    right = x > 0
    # bumpers wrap round the corners
    if iy <= 8 and (iz <= -36 or iz >= 36):
        return PLASTIC_HI if iy == 8 else PLASTIC
    if v_arch_edge(z, y):
        return Paint("body", -0.34)
    if iy <= 3:
        return Paint("body", -0.3)
    # rubbing strip along the doors
    if iy in (8, 9) and -18 <= iz <= 18:
        return PLASTIC_HI if iy == 9 else PLASTIC
    # front door, B-pillar, sliding door
    if iz == -28 and iy >= 12:
        return Paint("body", -0.5)
    if iz == -11 and iy <= 28:
        return Paint("body", -0.5)
    if right and iz in (-9, 12) and iy <= 28:
        return Paint("body", -0.5)
    if right and iy == 28 and -9 <= iz <= 12:
        return Paint("body", -0.5)
    if iy == 16 and (iz in (-14, -13) or (right and iz in (-7, -6))):
        return Paint("body", -0.42)
    if iy == 17 and (iz in (-14, -13) or (right and iz in (-7, -6))):
        return Paint("body", 0.22)
    # the sliding door's rail, behind it
    if right and 13 <= iz <= 35 and iy in (21, 22):
        return Paint("body", -0.48 if iy == 22 else 0.16)
    if iz == -35 and iy == 14:
        return AMBER
    # cab windows
    if iz <= -12 and 19 <= iy <= 28:
        if iy == 19 or iy == 28 or iz == -12:
            return GLOSS
        return glass(y, z, y, top=27, bottom=20)
    if iy >= 29:
        return Paint("body", 0.12)
    k = {18: 0.18, 17: 0.05, 11: -0.04, 10: -0.06, 7: -0.1, 6: -0.12, 5: -0.16, 4: -0.22}.get(iy, 0.0)
    if 19 <= iy <= 28:
        k = 0.04
    return Paint("body", k)


def van_front(c):
    """The nose's face between the bumper and the bonnet: a grille of chrome bars that
    runs into the headlamps."""
    a, iy = ax(c), math.floor(c.y)
    if iy >= 15:
        return Paint("body", 0.14)
    if iy == 14:
        return CHROME if a <= 8 else Paint("body", 0.06)
    if a <= 6 and 10 <= iy <= 13:
        if a == 6 or iy == 10:
            return PLASTIC_HI
        return CHROME_DARK if iy == 12 else GLOSS
    if iy == 9:
        return Paint("body", -0.1)
    return Paint("body", 0.0)


def van_bumper(c):
    y, z = c.y, c.z
    iy = math.floor(y)
    if c.top:
        return PLASTIC_HI
    if c.bottom:
        return UNDER
    if c.side:
        return PLASTIC_HI if iy == 8 else PLASTIC
    a = ax(c)
    front = c.front
    if iy == 8:
        return PLASTIC_HI
    if iy <= 3:
        return PLASTIC_LO
    if front:
        if iy in (4, 5) and a <= 7:
            return GLOSS if iy == 4 else hexc("#34363c")
        if iy == 5 and a in (11, 12):
            return FOG
        if iy in (4, 5, 6) and a in (10, 13) or (iy in (4, 6) and a in (11, 12)):
            return GLOSS
    else:
        if iy == 7 and a in (13, 14):
            return TAIL_DARK
    return PLASTIC


def paint_van_lower(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        return Paint("body", 0.16)
    if c.side:
        return van_side(c)
    if c.front:
        if z < -37.9:
            return van_front(c)
        if y < 12.5:
            return UNDER
        return Paint("body", 0.1)
    if c.back:
        if z > 37.9:
            return van_back(c)
        return UNDER
    return Paint("body", 0.0)


def van_back(c):
    """The rear doors, on the lower body and the glasshouse."""
    y = c.y
    a, iy = ax(c), math.floor(y)
    if a >= 14:
        return Paint("body", -0.12)
    if a == 13:
        return Paint("body", -0.5) if 10 <= iy <= 28 else Paint("body", -0.1)
    if a == 0:
        return Paint("body", -0.5) if iy <= 28 else Paint("body", 0.0)
    if iy >= 29:
        return Paint("body", 0.1)
    if 20 <= iy <= 27 and 2 <= a <= 11:
        if iy in (20, 27) or a in (2, 11):
            return GLOSS
        return glass(y, a, y, top=26, bottom=21)
    if iy == 15 and a in (1, 2):
        return Paint("body", -0.45)
    if iy == 16 and a in (1, 2):
        return Paint("body", 0.2)
    if iy in (12, 25) and a == 12:
        return PLASTIC
    if iy == 9:
        return Paint("body", -0.2)
    return Paint("body", -0.04)


def paint_van_cabin(c):
    if c.top:
        return Paint("body", 0.2)
    if c.bottom:
        return UNDER
    if c.side:
        return van_side(c)
    if c.back:
        return van_back(c)
    return Paint("body", 0.06)


def paint_van_ws(c):
    """The windscreen: glass on its upper face, black round the edge."""
    if c.face == "up":
        a = ax(c)
        lz = c.loc[2] - V_WS_BASE[0]
        if a >= 13 or lz < 1.5 or lz > c.box.to[2] - c.box.frm[2] - 1:
            return GLOSS
        if lz < 2.5 and a % 6 == 3:
            return PLASTIC_HI  # wiper arms
        return glass(c.y, xs(c), lz, top=29, bottom=19)
    return GLOSS


def paint_van_pillar(c):
    if c.face == "up":
        return Paint("body", 0.12)
    if c.side:
        return Paint("body", 0.04)
    return Paint("body", 0.0)


def paint_van_bonnet(c):
    if c.face == "up":
        a = ax(c)
        lz = c.loc[2] - V_BONNET[0]
        length = c.box.to[2] - c.box.frm[2]
        if lz > length - 1.6:
            return PLASTIC  # the cowl
        if lz < 1:
            return Paint("body", 0.2)
        if a == 9:
            return Paint("body", 0.2)
        if a == 10:
            return Paint("body", 0.02)
        return Paint("body", 0.1)
    if c.side:
        return Paint("body", 0.16) if c.loc[1] > V_BONNET[1] - 1 else Paint("body", 0.04)
    return Paint("body", 0.1)


def paint_van_roof(c):
    if c.top:
        iz = math.floor(c.z)
        if ax(c) <= 10 and iz % 9 == 0 and -12 <= iz <= 33:
            return Paint("body", -0.06)
        if ax(c) <= 10 and iz % 9 == 1 and -12 <= iz <= 33:
            return Paint("body", 0.16)
        return Paint("body", 0.08)
    if c.bottom:
        return GLOSS
    if c.back:
        return Paint("body", 0.0)
    return Paint("body", 0.1)


def paint_van_headlight(c):
    """Big headlamps swept up to the bonnet: a light strip along the top, two lenses
    under it, the indicator at the outer end."""
    iy = math.floor(c.y)
    a = round(abs(c.x) - 0.1)
    if c.front:
        if a >= 15:
            return AMBER if iy in (12, 13, 14) else HOUSING
        if iy == 15:
            return DRL
        if iy in (13, 14):
            if a in (10, 13):
                return CHROME_DARK
            return LENS if a in (11, 12, 14) else HOUSING
        if iy == 12:
            return HOUSING if a >= 10 else DRL
        return HOUSING_DARK
    if c.side:
        return {15: DRL, 14: AMBER, 13: AMBER}.get(iy, HOUSING_DARK)
    return HOUSING


def paint_van_taillight(c):
    iy = math.floor(c.y)
    if c.back or c.side:
        if iy in (18, 19, 17):
            return TAIL_BRIGHT if iy != 17 else TAIL_MID
        if iy in (15, 16):
            return AMBER
        if iy == 14:
            return REVERSE
        if iy in (12, 13):
            return TAIL_MID
        return TAIL_DARK
    return TAIL_DARK


def paint_wheel_van(c):
    x, y, z = c.p
    wx, wy, wz = c.box.extra["wheel"]
    if c.side:
        if (c.n[0] > 0) != (wx > 0):
            return TIRE_SIDE
        r = math.hypot(z - wz, y - wy)
        # a steel wheel under a plain trim: hub, a ring of five slots, dark lip
        if r < 1.3:
            return HUB
        if r < 3.3:
            if 2.0 < r < 2.9:
                a = math.degrees(math.atan2(y - wy, z - wz)) % 72
                if min(a, 72 - a) < 13:
                    return RIM_DARK
            return RIM
        if r < 3.9:
            return RIM_LIP
        if r < 4.7:
            return TIRE_SIDE
        return TIRE
    return TREAD_GROOVE if math.floor(abs(x)) in (11, 13) else TREAD_RIB


def build_van():
    boxes = []
    add = boxes.append
    W = V_HW
    # Lower body, front to back; the wheel arches are the gaps between these.
    add(Box("front_bumper", (-W, 3, -V_HL), (W, 9, -38), "bumper"))
    add(Box("nose", (-W, 3, -38), (W, 16, -32), "lower"))
    add(Box("front_arch_top", (-W, 12, -32), (W, 16, -28), "lower"))
    add(Box("front_arch_top_cab", (-W, 12, -28), (W, V_BELT, -19), "lower"))
    add(Box("front_arch_fill_front", (-W, 10, -32), (W, 12, -31), "lower"))
    add(Box("front_arch_fill_rear", (-W, 10, -20), (W, 12, -19), "lower"))
    add(Box("middle", (-W, 3, -19), (W, V_BELT, 19), "lower"))
    add(Box("rear_arch_top", (-W, 12, 19), (W, V_BELT, 32), "lower"))
    add(Box("rear_arch_fill_front", (-W, 10, 19), (W, 12, 20), "lower"))
    add(Box("rear_arch_fill_rear", (-W, 10, 31), (W, 12, 32), "lower"))
    add(Box("rear_quarter", (-W, 3, 32), (W, V_BELT, 38), "lower"))
    add(Box("rear_bumper", (-W, 3, 38), (W, 9, V_HL), "bumper"))
    add(Box("sills", (-W + 0.5, 2, -19), (W - 0.5, 3, 19), "trim"))
    add(Box("underbody", (-10.5, 2.5, -37), (10.5, 12, 37), "under"))
    add(slab("bonnet", V_BONNET, V_WS_BASE, 15.1, 3, "bonnet"))

    # Glasshouse and load box: level steps carry the side glass up the windscreen's
    # slope; the windscreen is glass between two pillar strips, so the rider looks out
    # through the steps with nothing drawn in front of them.
    (bz, by), (tz, ty) = V_WS_BASE, V_WS_TOP
    for k in range(V_BELT, 30):
        z0 = bz + (k + 1 - by) * (tz - bz) / (ty - by)
        if z0 >= tz - 0.01:
            break
        add(Box(f"cab_step_{k}", (-V_CAB, k, z0), (V_CAB, k + 1, tz), "cabin"))
    add(Box("load_box", (-V_CAB, V_BELT, tz), (V_CAB, 30, 38), "cabin"))
    add(slab("windscreen", V_WS_BASE, V_WS_TOP, V_CAB - 0.25, 1.5, "windscreen"))
    boxes += pair(slab("a_pillar_right", V_WS_BASE, V_WS_TOP, (V_CAB - 0.25, V_CAB + 0.25), 1.5, "pillar"))
    add(Box("roof", (-V_ROOF, 30, tz - 0.5), (V_ROOF, V_TOP, 37.5), "roof"))

    boxes += pair(Box("mirror_right", (V_HW + 0.3, 20, -27), (V_HW + 3.3, 25, -25), "mirror"))
    boxes += pair(Box("mirror_arm_right", (V_CAB, 20, -26.5), (V_HW + 0.3, 21, -25.5), "trim"))

    boxes += pair(Box("headlight_right", (8.6, 11, -38.25), (15.6, 16, -38), "headlight", lamp="head"))
    boxes += pair(Box("headlight_side_right", (15.6, 12, -38), (15.85, 16, -35.5), "headlight", lamp="head"))
    boxes += pair(Box("taillight_right", (12.6, 11, 38), (15.6, 20, 38.25), "taillight", lamp="brake"))
    boxes += pair(Box("taillight_side_right", (15.6, 11, 36), (15.85, 20, 38), "taillight", lamp="brake"))
    add(Box("brake_light_top", (-3.5, 29, 38), (3.5, 30, 38.25), "taillight", lamp="brake"))
    add(Box("plate_front", (-4.5, 6, -V_HL - 0.25), (4.5, 8, -V_HL), "plate_front"))
    add(Box("plate_rear", (-4.5, 10, 38), (4.5, 12, 38.25), "plate_rear"))

    bones = {}
    for zname, wz in (("front", -V_WZ), ("rear", V_WZ)):
        boxes += wheel_boxes(bones, zname, (12.5, V_WR, wz), 10.5, 14.6,
                             [(5.5, 3.5, 0.25), (4.5, 4.5, 0.0), (3.5, 5.5, 0.25)], "wheel")

    painters = {
        "lower": paint_van_lower,
        "bumper": van_bumper,
        "trim": lambda c: UNDER if c.bottom else TRIM,
        "under": lambda c: UNDER,
        "bonnet": paint_van_bonnet,
        "cabin": paint_van_cabin,
        "windscreen": paint_van_ws,
        "pillar": paint_van_pillar,
        "roof": paint_van_roof,
        "mirror": mirror_painter(V_HW + 3.3),
        "headlight": paint_van_headlight,
        "taillight": paint_van_taillight,
        "plate_front": plate(False),
        "plate_rear": plate(True),
        "wheel": paint_wheel_van,
    }
    return Model(
        "car_van", boxes, painters, {"body": "minecraft:white_concrete"},
        bones=bones, eye=(0.0, 1.42 * M, -1.25 * M),
    )


# ---------------------------------------------------------------- pickup

P_HL = 42.4            # half length, 5.3 m
P_HW = 15.6            # half width, 1.95 m
P_CAB = 14.6
P_ROOF = 13.6
P_BELT = 19
P_TOP = 28.8           # 1.8 m
P_WS_BASE = (-22.0, 19.0)
P_WS_TOP = (-10.0, 28.0)
P_BONNET = (-40.5, 17.0)
P_CAB_END = 13.0
P_BED = 13.5           # where the load bed starts
P_FLOOR = 12           # the bed floor
P_GATE = 40.4          # inside face of the tailgate
P_WZ, P_WR = 26.0, 6.0  # axles 1.0 m from the ends, wheels 0.74 m
LINER = hexc("#2a2b2f")
LINER_RIB = hexc("#35373c")
LINER_DARK = hexc("#1f2023")


def p_arch_open(z, y):
    for wz in (-P_WZ, P_WZ):
        lo, hi = wz - 8, wz + 8
        if lo < z < hi and y < 14:
            if y >= 12 and (z < lo + 1 or z > hi - 1):
                return False
            return True
    return False


def p_arch_edge(z, y):
    return not p_arch_open(z, y) and (p_arch_open(z, y - 1) or p_arch_open(z - 1, y) or p_arch_open(z + 1, y))


def pickup_side(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    if iy <= 10 and (iz <= -41 or iz >= 41):
        return PLASTIC
    if p_arch_edge(z, y):
        return PLASTIC_HI if p_arch_open(z, y - 1) else PLASTIC
    if iy <= 5:
        return Paint("body", -0.32)
    # door gaps, handles, the gap between cab and bed
    if iz == -22 and iy >= 14:
        return Paint("body", -0.5)
    if iz in (-3, 11) and iy <= 27:
        return Paint("body", -0.5)
    if iy == 16 and iz in (-7, -6, 7, 8):
        return CHROME_DARK
    if iy == 17 and iz in (-7, -6, 7, 8):
        return CHROME
    if iz == -37 and iy == 15:
        return AMBER
    # cab windows: front and rear doors, the thick C-pillar behind
    if iz <= 9 and 19 <= iy <= 27:
        if iy in (19, 27) or iz in (-4, -2, 9):
            return GLOSS
        if iz == -3:
            return GLOSS
        return glass(y, z, y, top=26, bottom=20)
    if iy >= 28:
        return Paint("body", 0.14)
    # a crease along the bed and doors, a shoulder at the belt
    k = {18: 0.2, 17: 0.06, 13: 0.12, 12: -0.1, 11: -0.04, 8: -0.06, 7: -0.1, 6: -0.18}.get(iy, 0.0)
    if 19 <= iy <= 27:
        k = 0.06
    return Paint("body", k)


def pickup_front(c):
    """The nose's face: a tall grille of thick bars between the headlamps."""
    a, iy = ax(c), math.floor(c.y)
    if iy >= 16 and a > 9:
        return Paint("body", 0.12)
    if a <= 9 and 10 <= iy <= 16:
        if a == 9 or iy in (10, 16):
            return CHROME if iy == 16 or a == 9 else CHROME_DARK
        if iy in (12, 14):
            return PLASTIC_HI if a % 3 else CHROME_DARK
        return GLOSS
    if iy <= 9:
        return Paint("body", -0.1)
    return Paint("body", 0.02)


def pickup_bumper(c):
    iy = math.floor(c.y)
    if c.top:
        return Paint("body", 0.12)
    if c.bottom:
        return UNDER
    if c.side:
        return Paint("body", 0.0) if iy >= 9 else PLASTIC
    a = ax(c)
    if c.front:
        if iy >= 9:
            return Paint("body", 0.04 if iy == 10 else -0.04)
        if iy <= 4:
            return STEEL if a <= 10 else PLASTIC_LO  # skid plate
        if iy in (6, 7) and 12 <= a <= 13:
            return FOG
        if 5 <= iy <= 8 and 11 <= a <= 14:
            return GLOSS
        if iy in (5, 6, 7) and a <= 8:
            return GLOSS if iy != 6 else PLASTIC_HI
        return PLASTIC
    return PLASTIC


def paint_pickup_lower(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        if z > P_BED and abs(x) > P_CAB - 0.1:
            return PLASTIC  # rail caps along the bed walls
        return Paint("body", 0.16)
    if c.side:
        if (x > 0) != (c.n[0] > 0):
            return LINER  # inside the bed
        return pickup_side(c)
    if c.front:
        if z < -40.4:
            return pickup_front(c)
        if z > P_BED + 0.5:
            return LINER
        if y < 14.5:
            return UNDER
        return Paint("body", 0.1)
    if c.back:
        if z > P_GATE - 0.1:
            return Paint("body", -0.06)
        if P_CAB_END - 1 < z < P_BED + 1.1 and y > P_FLOOR:
            return LINER if z > P_CAB_END + 0.1 else Paint("body", -0.08)
        return UNDER
    return Paint("body", 0.0)


def paint_pickup_rear_bumper(c):
    """A chrome step bumper with a black tread in the middle."""
    iy = math.floor(c.y)
    if c.bottom:
        return UNDER
    if c.top:
        return PLASTIC_HI if ax(c) <= 7 else CHROME
    if c.back:
        a = ax(c)
        if iy == 8:
            return CHROME
        if iy == 4:
            return CHROME_DARK
        if iy == 5 and a <= 2:
            return GLOSS  # the tow hitch's socket
        return RIM if iy == 7 else RIM_DARK if iy == 5 else STEEL
    return CHROME_DARK if iy < 7 else CHROME


def paint_pickup_floor(c):
    if c.top:
        a = ax(c)
        return LINER_RIB if a % 3 == 1 and a <= 13 else LINER
    if c.back or c.front or c.side:
        return LINER if c.y > P_FLOOR - 0.1 else UNDER
    return UNDER


def paint_pickup_tailgate(c):
    iy = math.floor(c.y)
    if c.top:
        return PLASTIC
    if c.front:
        return LINER
    if c.back:
        a = ax(c)
        if iy == 18:
            return Paint("body", 0.18)
        if iy == 16 and a <= 2:
            return GLOSS  # the handle
        if iy == 15 and a <= 2:
            return Paint("body", -0.3)
        if iy == 9:
            return Paint("body", -0.18)
        if a == 0 and iy == 13:
            return Paint("body", 0.12)
        return Paint("body", -0.04 if iy < 14 else 0.0)
    return Paint("body", 0.0)


def paint_pickup_cabin(c):
    x, y, z = c.p
    if c.top:
        return Paint("body", 0.2)
    if c.bottom:
        return UNDER
    if c.side:
        return pickup_side(c)
    if c.back:
        a, iy = ax(c), math.floor(y)
        if 21 <= iy <= 26 and a <= 9:
            if iy in (21, 26) or a == 9:
                return GLOSS
            if a == 0:
                return PLASTIC  # the sliding middle pane's frame
            return glass(y, a, y, top=25, bottom=22)
        if iy >= 27:
            return Paint("body", 0.04)
        return Paint("body", -0.06)
    return Paint("body", 0.06)


def paint_pickup_ws(c):
    if c.face == "up":
        a = ax(c)
        lz = c.loc[2] - P_WS_BASE[0]
        if a >= 13 or lz < 1.5 or lz > c.box.to[2] - c.box.frm[2] - 1:
            return GLOSS
        if lz < 2.5 and a % 6 == 3:
            return PLASTIC_HI
        return glass(c.y, xs(c), lz, top=27, bottom=20)
    return GLOSS


def paint_pickup_bonnet(c):
    if c.face == "up":
        a = ax(c)
        lz = c.loc[2] - P_BONNET[0]
        length = c.box.to[2] - c.box.frm[2]
        if lz > length - 1.6:
            return PLASTIC
        if lz < 1:
            return Paint("body", 0.2)
        if a in (6, 7):
            return Paint("body", 0.2 if a == 6 else 0.0)  # the power bulge's edges
        if a < 6:
            return Paint("body", 0.14)
        return Paint("body", 0.08)
    if c.side:
        return Paint("body", 0.16) if c.loc[1] > P_BONNET[1] - 1 else Paint("body", 0.04)
    return Paint("body", 0.1)


def paint_pickup_roof(c):
    if c.top:
        return Paint("body", 0.16 if ax(c) >= 13 else 0.1)
    if c.bottom:
        return GLOSS
    return Paint("body", 0.1)


def paint_pickup_headlight(c):
    iy = math.floor(c.y)
    a = round(abs(c.x) - 0.1)
    if c.front:
        if a >= 15:
            return AMBER if iy == 15 else HOUSING
        if iy == 16:
            return DRL if a >= 11 else HOUSING
        if iy == 15:
            return LENS if a in (12, 14) else (CHROME if a == 13 else HOUSING)
        return DRL if a <= 13 else HOUSING_DARK
    if c.side:
        return {16: DRL, 15: AMBER}.get(iy, HOUSING_DARK)
    return HOUSING


def paint_pickup_taillight(c):
    iy = math.floor(c.y)
    if c.back or c.side:
        if iy >= 17:
            return TAIL_BRIGHT
        if iy == 16:
            return TAIL_MID
        if iy in (14, 15):
            return AMBER if iy == 15 else REVERSE
        if iy in (12, 13):
            return TAIL_BRIGHT if iy == 13 else TAIL_MID
        return TAIL_DARK
    return TAIL_DARK


def alloy(dz, dy, hub, rim, lip, side, spokes=5, face=RIM, gap=RIM_DARK, centre=HUB):
    """A wheel's outer face: hub, `spokes` spokes out to `rim`, a lip to `lip`, the tyre's
    sidewall to `side` and its tread beyond."""
    r = math.hypot(dz, dy)
    if r < hub:
        return centre
    if r < rim:
        step = 360 / spokes
        a = (math.degrees(math.atan2(dy, dz)) + 90) % step
        return face if min(a, step - a) < step * 0.28 else gap
    if r < lip:
        return RIM_LIP
    if r < side:
        return TIRE_SIDE
    return TIRE


def paint_wheel_pickup(c):
    """Off-road tyres with tall sidewalls and shoulder lugs, on six-spoke alloys."""
    x, y, z = c.p
    wx, wy, wz = c.box.extra["wheel"]
    if c.side:
        if (c.n[0] > 0) != (wx > 0):
            return TIRE_SIDE
        return alloy(z - wz, y - wy, 1.2, 3.4, 4.1, 5.0, spokes=6, face=hexc("#a4abb4"))
    along = c.loc[2] if (c.top or c.bottom) else c.loc[1]
    lane = math.floor(abs(x)) - 10
    if lane in (1, 3):
        return TREAD_GROOVE
    if lane in (0, 4) and math.floor(along) % 2 == 0:
        return TREAD_GROOVE
    return TREAD_RIB


def build_pickup():
    boxes = []
    add = boxes.append
    W = P_HW
    fa, ra = -P_WZ, P_WZ
    add(Box("front_bumper", (-W, 3, -P_HL), (W, 11, -40.5), "bumper"))
    add(Box("nose", (-W, 5, -40.5), (W, 17, -34), "lower"))
    add(Box("front_arch_top", (-W, 14, -34), (W, 17, -22), "lower"))
    add(Box("front_arch_top_cab", (-W, 14, -22), (W, P_BELT, -18), "lower"))
    add(Box("front_arch_fill_front", (-W, 12, -34), (W, 14, -33), "lower"))
    add(Box("front_arch_fill_rear", (-W, 12, -19), (W, 14, -18), "lower"))
    add(Box("cab_lower", (-W, 5, -18), (W, P_BELT, P_CAB_END), "lower"))
    add(Box("cab_bed_gap", (-W + 0.5, 6, P_CAB_END - 0.5), (W - 0.5, P_BELT - 0.5, P_BED + 0.5), "under"))
    # The load bed: walls round an open box, floor at P_FLOOR, arch housings inside.
    add(Box("bed_front", (-W, 5, P_BED), (W, P_BELT, P_BED + 1), "lower"))
    add(Box("bed_under_front", (-W, 5, P_BED + 1), (W, P_FLOOR - 1, 18), "lower"))
    add(Box("bed_under_rear", (-W, 5, 34), (W, P_FLOOR - 1, P_GATE), "lower"))
    boxes += pair(Box("bed_wall_front_right", (P_CAB, P_FLOOR - 1, P_BED + 1), (W, P_BELT, 18), "lower"))
    boxes += pair(Box("bed_arch_top_right", (P_CAB, 14, 18), (W, P_BELT, 34), "lower"))
    boxes += pair(Box("bed_arch_fill_front_right", (P_CAB, 12, 18), (W, 14, 19), "lower"))
    boxes += pair(Box("bed_arch_fill_rear_right", (P_CAB, 12, 33), (W, 14, 34), "lower"))
    boxes += pair(Box("bed_wall_rear_right", (P_CAB, P_FLOOR - 1, 34), (W, P_BELT, P_GATE), "lower"))
    boxes += pair(Box("bed_post_right", (13.6, 9, P_GATE), (W, P_BELT, P_GATE + 1), "lower"))
    add(Box("bed_floor", (-P_CAB, P_FLOOR - 1, P_BED + 1), (P_CAB, P_FLOOR, P_GATE), "floor"))
    boxes += pair(Box("wheel_housing_right", (10.6, P_FLOOR, 19), (P_CAB, 15, 33), "floor"))
    add(Box("tailgate", (-13.6, 9, P_GATE), (13.6, P_BELT, P_GATE + 1), "tailgate"))
    add(Box("rear_bumper", (-W, 4, P_GATE + 1), (W, 9, P_HL), "rear_bumper"))
    add(Box("underbody", (-10.6, 3, -40), (10.6, 14, 40), "under"))
    boxes += pair(Box("step_right", (12.6, 4, -17), (W, 5, 17), "step"))
    add(slab("bonnet", P_BONNET, P_WS_BASE, 15.1, 3, "bonnet"))

    (bz, by), (tz, ty) = P_WS_BASE, P_WS_TOP
    for k in range(P_BELT, 29):
        z0 = bz + (k + 1 - by) * (tz - bz) / (ty - by)
        if z0 >= tz - 0.01:
            break
        add(Box(f"cab_step_{k}", (-P_CAB, k, z0), (P_CAB, k + 1, tz), "cabin"))
    add(Box("cabin", (-P_CAB, P_BELT, tz), (P_CAB, 28, P_CAB_END), "cabin"))
    add(slab("windscreen", P_WS_BASE, P_WS_TOP, P_CAB - 0.25, 1.5, "windscreen"))
    boxes += pair(slab("a_pillar_right", P_WS_BASE, P_WS_TOP, (P_CAB - 0.25, P_CAB + 0.25), 1.5, "pillar"))
    add(Box("roof", (-P_ROOF, 28, tz - 0.5), (P_ROOF, P_TOP, P_CAB_END - 0.5), "roof"))

    boxes += pair(Box("mirror_right", (W + 0.3, 20, -20), (W + 3.3, 23.5, -18), "mirror"))
    boxes += pair(Box("mirror_arm_right", (P_CAB, 20, -19.5), (W + 0.3, 21, -18.5), "trim"))
    boxes += pair(Box("headlight_right", (10.6, 13, -40.75), (15.6, 17, -40.5), "headlight", lamp="head"))
    boxes += pair(Box("headlight_side_right", (15.6, 14, -40.5), (15.85, 17, -38), "headlight", lamp="head"))
    boxes += pair(Box("taillight_right", (13.6, 11, P_GATE + 1), (15.6, 19, P_GATE + 1.25), "taillight", lamp="brake"))
    add(Box("brake_light_top", (-3.5, 27, P_CAB_END), (3.5, 27.5, P_CAB_END + 0.25), "taillight", lamp="brake"))
    add(Box("plate_front", (-4.5, 7, -P_HL - 0.25), (4.5, 9, -P_HL), "plate_front"))
    add(Box("plate_rear", (-4.5, 10, P_GATE + 1), (4.5, 12, P_GATE + 1.25), "plate_rear"))

    bones = {}
    for zname, wz in (("front", fa), ("rear", ra)):
        boxes += wheel_boxes(bones, zname, (13.0, P_WR, wz), 10.6, 15.35,
                             [(6.0, 3.5, 0.25), (4.75, 4.75, 0.0), (3.5, 6.0, 0.25)], "wheel")

    painters = {
        "lower": paint_pickup_lower,
        "bumper": pickup_bumper,
        "rear_bumper": paint_pickup_rear_bumper,
        "trim": lambda c: UNDER if c.bottom else TRIM,
        "step": lambda c: PLASTIC_HI if c.top else PLASTIC,
        "under": lambda c: UNDER,
        "floor": paint_pickup_floor,
        "tailgate": paint_pickup_tailgate,
        "bonnet": paint_pickup_bonnet,
        "cabin": paint_pickup_cabin,
        "windscreen": paint_pickup_ws,
        "pillar": paint_van_pillar,
        "roof": paint_pickup_roof,
        "mirror": mirror_painter(W + 3.3),
        "headlight": paint_pickup_headlight,
        "taillight": paint_pickup_taillight,
        "plate_front": plate(False),
        "plate_rear": plate(True),
        "wheel": paint_wheel_pickup,
    }
    return Model(
        "car_pickup", boxes, painters, {"body": "minecraft:red_concrete"},
        bones=bones, eye=(0.0, 1.28 * M, -0.95 * M),
    )


# ---------------------------------------------------------------- truck

T_HL = 60.8            # half length, 7.6 m
T_HW = 19.2            # half width of the box body, 2.4 m
T_CAB = 18.5           # the cab, a little narrower
T_CAB_FRONT = -57.0    # the cab's shell behind the windscreen and grille panel
T_CAB_BACK = -30.5
T_ROOF = 46.0          # cab roof, 2.9 m
T_BOX_FRONT = -29.0
T_BOX_TOP = 51.2       # 3.2 m
T_BOX_FLOOR = 16.5
T_WS_BASE = (-59.5, 28.0)
T_WS_TOP = (-57.5, 42.5)
T_WZ, T_WR = 40.5, 7.5  # axles 1.25 m from the ends, wheels 0.95 m
ALU = hexc("#e3e6e9")
ALU_SHADE = hexc("#cdd1d6")
ALU_DARK = hexc("#a2a8b0")
ALU_HI = hexc("#eef0f2")
ALU_LINE = hexc("#8d939b")
WARN_RED = hexc("#c4202b")
WARN_YELLOW = hexc("#f2c230")


def truck_cab_side(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    right = x > 0
    if iy <= 16 and -50 <= iz <= -32:
        return PLASTIC  # the arch's edge
    if iy >= 45:
        return Paint("body", 0.16)
    if iy == 44:
        return Paint("body", -0.12)
    # the corner deflector, the door, the panel behind it
    if iz == -55 and iy >= 15:
        return Paint("body", -0.45)
    if iz in (-54, -39) and 15 <= iy <= 43:
        return Paint("body", -0.5)
    if iy == 43 and -54 <= iz <= -39:
        return Paint("body", -0.5)
    if 31 <= iy <= 41 and -53 <= iz <= -41:
        if iy in (31, 41) or iz in (-53, -41, -49):
            return GLOSS
        return glass(y, z, y, top=40, bottom=32)
    if right and 20 <= iy <= 26 and -52 <= iz <= -47:
        if iy in (20, 26) or iz in (-52, -47):
            return GLOSS
        return glass(y, z, y, top=25, bottom=21)
    if iy == 28 and iz in (-43, -42):
        return GLOSS
    if iy == 29 and iz in (-43, -42):
        return Paint("body", 0.25)
    # an air intake behind the door
    if not right and 36 <= iy <= 41 and -36 <= iz <= -33:
        return GLOSS if iy % 2 == 0 else PLASTIC_HI
    if iy <= 15:
        return Paint("body", -0.28)
    k = 0.04 if iy >= 30 else -0.02
    if iy in (17, 16):
        k = -0.16
    return Paint("body", k)


def truck_grille(c):
    """The cab's face under the windscreen: a wide grille of body-coloured bars."""
    a, iy = ax(c), math.floor(c.y)
    if iy >= 26:
        return Paint("body", 0.18 if iy == 27 else 0.06)
    if a <= 13 and 17 <= iy <= 25:
        if a == 13 or iy in (17, 25):
            return PLASTIC
        return Paint("body", -0.2) if iy % 2 == 0 else GLOSS
    if iy <= 15:
        return Paint("body", -0.2 if iy == 14 else -0.12)
    if a >= 17:
        return Paint("body", -0.1)
    return Paint("body", 0.0)


def paint_truck_cab(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        return Paint("body", 0.12 if abs(x) < 17 else 0.2)
    if c.side:
        return truck_cab_side(c)
    if c.front:
        if z < -59.4:
            return truck_grille(c)
        if y > 44:
            return Paint("body", 0.1)
        if y < 17.5:
            return UNDER
        return Paint("body", 0.04)  # the corner strips beside the windscreen
    if c.back:
        if y < 17.5 and z < -31:
            return UNDER
        return Paint("body", -0.22)
    return Paint("body", 0.0)


def paint_truck_ws(c):
    if c.front:
        a = ax(c)
        ly = c.loc[1] - T_WS_BASE[1]
        height = c.box.to[1] - c.box.frm[1]
        if a >= 17:
            return Paint("body", 0.06)
        if a == 16 or ly < 1 or ly > height - 1:
            return GLOSS
        if ly < 2 and (3 <= a <= 13):
            return PLASTIC_HI  # wipers at rest
        return glass(c.y, xs(c), ly, top=41, bottom=29)
    if c.side:
        return Paint("body", 0.0)
    return GLOSS


def paint_truck_visor(c):
    if c.top:
        return Paint("body", 0.14)
    if c.bottom:
        return GLOSS
    if c.front:
        a = ax(c)
        if a in (2, 6, 10, 14) and math.floor(c.y) == 43:
            return AMBER  # clearance lamps
        return Paint("body", 0.06)
    return Paint("body", 0.0)


def paint_truck_spoiler(c):
    if c.face == "up":
        a = ax(c)
        return Paint("body", 0.2 if a >= 16 else 0.1)
    if c.side:
        return Paint("body", 0.14) if c.loc[1] > T_ROOF - 1 else Paint("body", -0.02)
    if c.back or c.face == "south":
        return Paint("body", -0.3)
    return Paint("body", 0.0)


def paint_truck_bumper(c):
    iy = math.floor(c.y)
    if c.top:
        return PLASTIC_HI
    if c.bottom:
        return UNDER
    if c.front:
        a = ax(c)
        if iy == 14:
            return PLASTIC_HI
        if iy <= 6:
            return PLASTIC_LO
        if iy in (7, 8) and 8 <= a <= 10:
            return FOG if iy == 8 else GLOSS
        if 7 <= iy <= 9 and a <= 5:
            return GLOSS if iy != 8 else PLASTIC_LO
        return PLASTIC
    return PLASTIC


def paint_truck_steps(c):
    iy = math.floor(c.y)
    if c.top:
        return PLASTIC_HI
    if c.side:
        if iy in (6, 10):
            return STEEL
        if iy in (7, 8, 11, 12):
            return GLOSS
        return PLASTIC
    return PLASTIC


def paint_box(c):
    """The aluminium box body, its walls between the ribs, and its rear doors."""
    x, y, z = c.p
    iy = math.floor(y)
    if c.bottom:
        return UNDER
    if c.top:
        return ALU_SHADE
    if c.side:
        if iy == 48:
            return ALU_SHADE
        return ALU if iy >= 21 else ALU_SHADE
    if c.back:
        a = ax(c)
        if a == 0:
            return ALU_LINE
        if a in (4, 13):
            return CHROME  # locking bars
        if a in (5, 14):
            return ALU_DARK
        if iy == 32 and a in (6, 7, 15, 16):
            return GLOSS
        if iy in (21, 22, 44, 45) and a in (3, 4, 5, 12, 13, 14):
            return STEEL  # the bars' keepers
        return ALU if iy >= 21 else ALU_SHADE
    return ALU


def paint_box_frame(c):
    """Rails and corner posts."""
    iy = math.floor(c.y)
    if c.bottom:
        return UNDER
    if c.top:
        a = ax(c)
        return ALU_HI if a >= 18 else ALU_SHADE
    if c.back and c.box.frm[1] > 18 and c.box.to[1] < 50:
        if iy in (23, 34, 45):
            return ALU_LINE  # hinges
        return ALU_SHADE
    if c.side or c.front or c.back:
        if c.box.frm[1] < 17:
            # the floor rail, with amber side markers
            iz = math.floor(c.z)
            if c.side and iy == 17 and iz in (-20, 2, 24, 46):
                return AMBER
            return ALU_DARK if iy == 16 else ALU_SHADE
        if c.box.frm[1] > 49:
            iz = math.floor(c.z)
            if c.front and iy == 50 and ax(c) in (17, 18):
                return DRL
            if c.back and iy == 50 and ax(c) in (17, 18):
                return TAIL_BRIGHT
            return ALU_HI if iy == 51 else ALU_SHADE
        return ALU_HI if c.side else ALU_SHADE
    return ALU


def paint_rib(c):
    if c.side:
        return ALU_HI
    return ALU_DARK if c.back else ALU_SHADE


def paint_underrun(c):
    a, iy = ax(c), math.floor(c.y)
    if c.back:
        if a >= 9:
            return WARN_RED if (math.floor(c.x) + iy) % 4 < 2 else WARN_YELLOW
        return PLASTIC
    if c.top:
        return PLASTIC_HI
    return PLASTIC


def paint_lampbar(c):
    return PLASTIC if not c.top else PLASTIC_HI


def paint_truck_tail(c):
    a = round(abs(c.x) - 0.5)
    iy = math.floor(c.y)
    if c.back:
        if a >= 16:
            return AMBER
        if a in (14, 15):
            return TAIL_BRIGHT
        if a == 13:
            return REVERSE
        return TAIL_MID if iy == 10 else TAIL_BRIGHT
    return TAIL_DARK


def paint_truck_headlight(c):
    a = round(abs(c.x) - 0.5)
    iy = math.floor(c.y)
    if c.front:
        if a >= 17:
            return AMBER if iy in (11, 12) else HOUSING
        if iy == 13:
            return DRL
        if iy in (11, 12):
            return CHROME_DARK if a == 14 else LENS
        if iy == 10 and a <= 13:
            return DRL
        return HOUSING_DARK
    if c.side:
        return AMBER if iy in (11, 12) else HOUSING
    return HOUSING


def paint_guard(c):
    if c.top:
        return CHROME
    if c.bottom:
        return ALU_DARK
    if c.side:
        return ALU_HI if math.floor(c.y) % 4 in (0, 3) else ALU_SHADE
    return ALU_SHADE


def paint_tank(c):
    iy, iz = math.floor(c.y), math.floor(c.z)
    if c.side:
        if iz in (-25, -14):
            return STEEL  # straps
        return {13: CHROME, 12: RIM_LIP, 6: CHROME_DARK, 5: STEEL}.get(iy, RIM)
    if c.top:
        return CHROME if iz != -20 else GLOSS
    return CHROME_DARK


def paint_flap(c):
    if c.back or c.front:
        iy = math.floor(c.y)
        if iy == 4:
            return STEEL
        return PLASTIC
    return PLASTIC_LO


def paint_wheel_truck(c):
    """Steel disc wheels with hand holes and a hub; ribbed tyres. The rear twins share
    their boxes, a groove where the two tyres meet."""
    x, y, z = c.p
    wx, wy, wz = c.box.extra["wheel"]
    twin = c.box.extra.get("twin")
    if c.side:
        if (c.n[0] > 0) != (wx > 0):
            return TIRE_SIDE
        r = math.hypot(z - wz, y - wy)
        if r < 1.6:
            return HUB if r < 1.0 else CHROME_DARK
        if r < 4.6:
            if 2.6 < r < 3.9:
                a = (math.degrees(math.atan2(y - wy, z - wz)) + 30) % 60
                if min(a, 60 - a) < 12:
                    return RIM_DARK
            return hexc("#c2c7cd") if not twin else hexc("#b5bbc2")
        if r < 5.2:
            return RIM_LIP if not twin else RIM_DARK
        if r < 6.4:
            return TIRE_SIDE
        return TIRE
    lane = math.floor(abs(x))
    if twin and lane == twin:
        return UNDER
    return TREAD_GROOVE if lane % 3 == 0 else TREAD_RIB


def paint_chassis(c):
    return UNDER if not c.side else hexc("#26272b")


def build_truck():
    boxes = []
    add = boxes.append
    W, C = T_HW, T_CAB
    # The cab: grille panel, the shell (cut for the front wheel), the windscreen leaning
    # back over it, visor and roof spoiler.
    add(Box("bumper", (-(C - 1), 6, -T_HL), (C - 1, 15, -59.5), "bumper"))
    boxes += pair(corner("bumper_corner_right", (C - 1, -T_HL), 1.0, 6.25, 14.75, "bumper"))
    add(Box("cab_face", (-(C - 1.5), 14.5, -59.5), (C - 1.5, 28, T_CAB_FRONT), "cab"))
    boxes += pair(corner("cab_corner_right", (C - 1.5, -59.5), 1.5, 14.5, 28, "cab"))
    add(Box("cab", (-C, 17, T_CAB_FRONT), (C, T_ROOF, T_CAB_BACK), "cab"))
    add(Box("cab_low_front", (-C, 14.5, T_CAB_FRONT), (C, 17, -49), "cab"))
    add(Box("cab_low_rear", (-C, 14.5, -32), (C, 17, T_CAB_BACK), "cab"))
    add(upright("windscreen", T_WS_BASE, T_WS_TOP, C - 0.75, 2.5, "windscreen"))
    add(Box("visor", (-(C - 1), 42.5, -59.0), (C - 1, 44, T_CAB_FRONT), "visor"))
    add(slab("spoiler", (-52.0, T_ROOF), (T_CAB_BACK - 0.5, 50.6), 16.6, 5, "spoiler"))
    boxes += pair(Box("steps_right", (15.5, 5, -58.5), (C, 14.5, -50), "steps"))
    boxes += pair(Box("mirror_arm_right", (C, 40, -56.25), (21.5, 41, -55.75), "trim"))
    boxes += pair(Box("mirror_right", (21.0, 31, -57), (22.5, 41.5, -55.5), "mirror"))
    boxes += pair(Box("mirror_low_right", (20.5, 27, -57), (22.0, 30, -55.5), "mirror"))
    boxes += pair(Box("headlight_right", (12.5, 9, -T_HL - 0.25), (C, 14, -T_HL), "headlight", lamp="head"))
    add(Box("plate_front", (-4.5, 10, -T_HL - 0.25), (4.5, 12, -T_HL), "plate_front"))

    # Chassis, tank, guards between the axles, mudflaps.
    add(Box("chassis", (-7, 8, -58), (7, 15, 58), "chassis"))
    add(Box("subframe", (-9, 15, T_CAB_BACK - 1), (9, T_BOX_FLOOR, 60), "chassis"))
    add(Box("wheelhouse", (-12.5, 13, -49), (12.5, 17.5, -32), "chassis"))
    add(Box("tank", (-17.5, 5, -27), (-11, 14, -12), "tank"))
    add(Box("battery_box", (11, 6, -26), (17.5, 13, -16), "chassis"))
    for k, (y0, y1) in enumerate(((6.5, 8.0), (11.0, 12.5))):
        boxes += pair(Box(f"guard_{k}_right", (18.2, y0, -31), (18.7, y1, 32), "guard"))
    boxes += pair(Box("flap_front_right", (12.5, 4, -31.75), (18.25, 14.5, -31.5), "flap"))
    boxes += pair(Box("flap_rear_right", (9.5, 3, 49.25), (18.6, 13, 49.5), "flap"))

    # The box body: walls set in a quarter unit from the rails and corner posts, ribs.
    add(Box("box", (-(W - 0.25), 19, T_BOX_FRONT + 0.25), (W - 0.25, 49.5, 60.5), "box"))
    add(Box("box_floor", (-W, T_BOX_FLOOR, T_BOX_FRONT), (W, 19, T_HL), "frame"))
    add(Box("box_roof", (-W, 49.5, T_BOX_FRONT), (W, T_BOX_TOP, T_HL), "frame"))
    boxes += pair(Box("post_front_right", (W - 1.5, 19, T_BOX_FRONT), (W, 49.5, T_BOX_FRONT + 1.5), "frame"))
    boxes += pair(Box("post_rear_right", (W - 1.5, 19, T_HL - 1.5), (W, 49.5, T_HL), "frame"))
    for k, z in enumerate(range(-19, 59, 9)):
        boxes += pair(Box(f"rib_{k}_right", (W - 0.25, 19, z), (W, 49.5, z + 1), "rib"))
    add(Box("lamp_bar", (-C, 9, 58.0), (C, 13.5, 59.75), "lampbar"))
    add(Box("underrun", (-17.5, 4.5, 58.5), (17.5, 8.5, 60.5), "underrun"))
    boxes += pair(Box("taillight_right", (11.0, 9.5, 59.75), (18.0, 12.5, 60.0), "taillight", lamp="brake"))
    add(Box("plate_rear", (-4.5, 10, 59.75), (4.5, 12, 60.0), "plate_rear"))

    bones = {}
    disc = [(7.5, 2.5, 0.5), (6.5, 4.5, 0.25), (4.5, 6.5, 0.25), (2.5, 7.5, 0.5), (5.5, 5.5, 0.0)]
    boxes += wheel_boxes(bones, "front", (15.25, T_WR, -T_WZ), 12.5, 18.0, disc)
    rear = wheel_boxes(bones, "rear", (14.0, T_WR, T_WZ), 9.5, 18.6, disc)
    for b in rear:
        b.extra["twin"] = 14
    boxes += rear

    painters = {
        "cab": paint_truck_cab,
        "windscreen": paint_truck_ws,
        "visor": paint_truck_visor,
        "spoiler": paint_truck_spoiler,
        "bumper": paint_truck_bumper,
        "steps": paint_truck_steps,
        "trim": lambda c: UNDER if c.bottom else TRIM,
        "mirror": mirror_painter(22.5),
        "headlight": paint_truck_headlight,
        "plate_front": plate(False),
        "plate_rear": plate(True),
        "chassis": paint_chassis,
        "tank": paint_tank,
        "guard": paint_guard,
        "flap": paint_flap,
        "box": paint_box,
        "frame": paint_box_frame,
        "rib": paint_rib,
        "lampbar": paint_lampbar,
        "underrun": paint_underrun,
        "taillight": paint_truck_tail,
        "wheel": paint_wheel_truck,
    }
    return Model(
        "car_truck", boxes, painters, {"body": "minecraft:blue_concrete"},
        bones=bones, eye=(0.0, 1.95 * M, -2.7 * M),
    )


# ---------------------------------------------------------------- bus

B_HL = 80.0            # half length, 10 m
B_HW = 20.0            # half width, 2.5 m
B_TOP = 48.0           # 3.0 m
B_END = 17.0           # half width of the front and rear faces, inside the chamfers
B_BAND = (17, 43)      # the black window band, bottom and top rows
B_WZ, B_WR = 45.0, 8.0  # axles 2.2 m from the ends, wheels 0.98 m
LED = hexc("#ffb21e")
LED_DIM = hexc("#3a2408")
DISPLAY = hexc("#141414")
BAND = hexc("#16171a")
B_DOORS = ((-76, -58), (-9, 9))
B_PILLARS_RIGHT = (-57, -41, -25, -10, 10, 27, 44, 61, 77)
B_PILLARS_LEFT = (-62, -46, -30, -14, 2, 18, 34, 50, 66, 77)

FONT = {
    "1": [".#.", "##.", ".#.", "###"], "2": ["###", "..#", "##.", "###"],
    "C": ["###", "#..", "#..", "###"], "E": ["###", "##.", "#..", "###"],
    "N": ["#.#", "###", "###", "#.#"], "T": ["###", ".#.", ".#.", ".#."],
    "R": ["##.", "#.#", "##.", "#.#"], " ": ["..", "..", "..", ".."],
}


def screen_glass(y, a, b, top, bottom):
    """A big windscreen: the cars' glass, its glints further apart."""
    c = mix(GLASS_DARK, GLASS_MID, (y - bottom) / max(1.0, top - bottom))
    ia, ib = math.floor(a), math.floor(b)
    if (ia - ib) % 29 < 8:
        m = (ia + ib) % 21
        if m in (0, 1):
            return GLASS_HI
        if m == 3:
            return GLASS_LIGHT
    return c


def band_glass(y, a, b, top=41.0, bottom=19.0):
    """The bus's side glass: darker than a car's, nearly black low down, with a sparse
    glint now and then."""
    c = mix(BAND, GLASS_DARK, 0.35 + 0.65 * (y - bottom) / (top - bottom))
    ia, ib = math.floor(a), math.floor(b)
    if (ia - ib) % 31 < 6:
        m = (ia + ib) % 17
        if m == 0:
            return GLASS_LIGHT
        if m == 2:
            return GLASS_MID
    return c


def led_text(text, width):
    """Text in the display's 4-row font, centred in `width` columns."""
    rows = ["", "", "", ""]
    for ch in text:
        for r in range(4):
            rows[r] += FONT[ch][r] + "."
    rows = [r[:-1] for r in rows]
    pad = (width - len(rows[0])) // 2
    assert pad >= 0, text
    return ["." * pad + r + "." * (width - pad - len(r)) for r in rows]


DEST = led_text("12 CENTRE", 34)
ROUTE = led_text("12", 8)


def b_arch_open(z, y):
    for wz in (-B_WZ, B_WZ):
        lo, hi = wz - 9, wz + 9
        if lo < z < hi and y < 18:
            if y >= 16 and (z < lo + 1 or z > hi - 1):
                return False
            return True
    return False


def b_arch_edge(z, y):
    return not b_arch_open(z, y) and (b_arch_open(z, y - 1) or b_arch_open(z - 1, y) or b_arch_open(z + 1, y))


def bus_door(lz, y, z0, z1):
    """A double door: two glazed leaves with black frames, a rail across."""
    iy, iz = math.floor(y), math.floor(lz)
    mid = (z0 + z1) // 2
    if iy >= B_BAND[1] + 1:
        return None
    if iz in (z0, z1 - 1, mid - 1, mid) or iy in (4, B_BAND[1]) or iy == 26:
        return GLOSS if iz not in (mid - 1, mid) else PLASTIC
    if iy <= 9:
        return mix(GLASS_DARK, (0, 0, 0), 0.4)
    return band_glass(y, lz, y, top=41, bottom=10)


def bus_side(c):
    x, y, z = c.p
    iy, iz = math.floor(y), math.floor(z)
    right = x > 0
    if right:
        for z0, z1 in B_DOORS:
            if z0 <= iz < z1 and iy >= 4:
                out = bus_door(z, y, z0, z1)
                if out is not None:
                    return out
    if b_arch_edge(z, y):
        return PLASTIC
    if iy <= 3:
        return PLASTIC_LO
    if B_BAND[0] <= iy <= B_BAND[1]:
        pillars = B_PILLARS_RIGHT if right else B_PILLARS_LEFT
        if iy in (B_BAND[0], B_BAND[1]) or iz in pillars or iy == B_BAND[1] - 1:
            return BAND
        # top-hung vents in every other pane
        k = next((i for i, p in enumerate(pillars) if p > iz), 0)
        if k % 2 and iy == 37:
            return BAND
        return band_glass(y, z, y)
    if iy >= 46:
        return Paint("body", 0.14 if iy == 47 else 0.06)
    if iy >= 44:
        return Paint("body", 0.02)
    if iy == 10:
        return PLASTIC  # rubbing strip
    if iy == 11:
        return Paint("body", 0.14)
    k = {16: 0.1, 15: 0.04, 9: -0.06, 8: -0.08, 7: -0.1, 6: -0.13, 5: -0.17, 4: -0.22}.get(iy, 0.0)
    return Paint("body", k)


def bus_front(c, a, y):
    iy = math.floor(y)
    if iy <= 7:
        return PLASTIC_HI if iy == 7 else PLASTIC
    if iy <= 13:
        if a >= 16.5 and iy in (10, 11):
            return AMBER
        return Paint("body", 0.08 if iy == 13 else 0.0)
    if iy >= 46:
        return Paint("body", 0.14)
    if iy >= 42 or iy == 14 or a >= 16.5 and c.box.rot[1] == 0:
        return BAND
    if iy in (15, 16) and 2 <= a <= 13 and (iy == 15) == (a < 9):
        return PLASTIC_HI  # wipers
    return screen_glass(y, a, y, top=41, bottom=15)


def paint_bus_body(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    if c.top:
        return Paint("body", 0.1)
    if c.box.rot[1]:
        # chamfered corners: the windscreen and bands wrap round them
        if z < 0:
            return bus_front(c, 17.0, y)
        return bus_rear(c, 17.0, y)
    if c.side:
        return bus_side(c)
    if c.front:
        if z < -79.4:
            return bus_front(c, abs(xs(c)), y)
        return UNDER if y < 18.5 else Paint("body", 0.0)
    if c.back:
        if z > 79.9:
            return bus_rear(c, abs(xs(c)), y)
        return UNDER if y < 18.5 else Paint("body", 0.0)
    return Paint("body", 0.0)


def bus_rear(c, a, y):
    iy = math.floor(y)
    if iy <= 7:
        return PLASTIC_HI if iy == 7 else PLASTIC
    if a >= 17 and 8 <= iy <= 19:
        return TAIL_MID if iy >= 15 else (AMBER if iy in (13, 14) else TAIL_DARK)
    if iy <= 21:
        # the engine lid, with a grille in its lower half
        if a <= 13 and 9 <= iy <= 14:
            return GLOSS if iy % 2 else PLASTIC_HI
        if a <= 14.5 and iy in (8, 21) or a in (14.5, 14) and 8 <= iy <= 21:
            return Paint("body", -0.4)
        return Paint("body", -0.06)
    if 26 <= iy <= 43:
        if a <= 12 and 27 <= iy <= 41:
            return band_glass(y, a, y, top=40, bottom=28)
        return BAND
    if iy >= 46:
        return Paint("body", 0.1)
    return Paint("body", -0.02)


def paint_bus_display(text):
    def paint(c):
        if not (c.front or c.back):
            return DISPLAY
        col, row = c.i, c.j  # from the left and the top, as seen from outside
        if 0 <= row < 4 and 0 <= col < len(text[0]) and text[row][col] == "#":
            return LED
        return DISPLAY
    return paint


def paint_bus_headlight(c):
    """A long lamp unit: a light strip over two round lenses, the indicator outside."""
    a = round(abs(c.x) - 0.5)
    iy = math.floor(c.y)
    if c.front:
        if a >= 16:
            return AMBER if iy in (10, 11) else HOUSING
        if iy == 12:
            return DRL
        if iy in (10, 11):
            return LENS if a in (12, 13, 14) else (CHROME_DARK if a == 15 else HOUSING)
        return HOUSING_DARK
    return HOUSING


def paint_bus_tail(c):
    iy = math.floor(c.y)
    if c.back or c.side:
        if iy >= 16:
            return TAIL_BRIGHT
        if iy in (14, 15):
            return AMBER
        if iy == 13:
            return REVERSE
        return TAIL_BRIGHT if iy >= 10 else TAIL_MID
    return TAIL_DARK


def paint_bus_pod(c):
    """A roof pod: two fans under round grilles on top, vents along the sides."""
    x, y, z = c.p
    iz = math.floor(z)
    if c.top:
        a = ax(c)
        lz = z - c.box.frm[2]
        size = c.box.to[2] - c.box.frm[2]
        for fz in (math.floor(size * 0.3) + 0.5, math.floor(size * 0.7) + 0.5):
            r = math.hypot(xs(c), lz - fz)
            if r < 3.3:
                if r < 0.9:
                    return PLASTIC_HI
                return GLOSS if r > 2.3 else PLASTIC
        lz0 = z - c.box.frm[2]
        if a >= c.box.to[0] - 1 or lz0 < 1 or lz0 > size - 1:
            return Paint("body", 0.24)
        return Paint("body", 0.02)
    if c.side:
        if math.floor(y) == B_TOP + 1 and iz % 2 == 0:
            return PLASTIC
        return Paint("body", 0.06)
    return Paint("body", 0.02)


def build_bus():
    boxes = []
    add = boxes.append
    W, E = B_HW, B_END
    fz, rz = -B_HL + 3, B_HL - 3
    # Front and rear: faces inside chamfered corners; the windscreen wraps the corners,
    # leaning back half a unit from the bumper.
    add(Box("front_lower", (-E, 3, -B_HL), (E, 14, fz), "body"))
    add(Box("front", (-(E + 0.5), 14, -B_HL + 0.5), (E + 0.5, B_TOP, fz), "body"))
    boxes += pair(corner("front_corner_low_right", (E, -B_HL), 3, 3.25, 13.75, "body", depth=3.0))
    boxes += pair(corner("front_corner_right", (E + 0.5, -B_HL + 0.5), 2.5, 13.75, B_TOP - 0.25, "body", depth=3.0))
    add(Box("rear", (-E, 3, rz), (E, B_TOP, B_HL), "body"))
    boxes += pair(corner("rear_corner_right", (E, B_HL), 3, 3.25, B_TOP - 0.25, "body", depth=3.0, rear=True))
    # The body in bays, so the roof and floor share a few spots of texture; the lower
    # half is cut for the wheels.
    cuts = [fz, -54, -36, -18, 0, 18, 36, 54, rz]
    for k in range(len(cuts) - 1):
        z0, z1 = cuts[k], cuts[k + 1]
        add(Box(f"upper_{k}", (-W, 18, z0), (W, B_TOP, z1), "body"))
        if z0 not in (-54, 36):
            add(Box(f"lower_{k}", (-W, 3, z0), (W, 18, z1), "body"))
        else:
            add(Box(f"wheelhouse_{k}", (-10, 3, z0), (10, 18, z1), "under"))
    for wz in (-B_WZ, B_WZ):
        add(Box(f"arch_fill_{int(wz)}_front", (-W, 16, wz - 9), (W, 18, wz - 8), "body"))
        add(Box(f"arch_fill_{int(wz)}_rear", (-W, 16, wz + 8), (W, 18, wz + 9), "body"))

    # Roof: equipment pods.
    add(Box("ac_pod", (-12.5, B_TOP, -50), (12.5, B_TOP + 3, -26), "pod"))
    add(Box("rear_pod", (-10.5, B_TOP, 50), (10.5, B_TOP + 2.5, 74), "pod"))
    add(Box("hatch_front", (-4.5, B_TOP, -16), (4.5, B_TOP + 0.5, -8), "hatch"))
    add(Box("hatch_rear", (-4.5, B_TOP, 20), (4.5, B_TOP + 0.5, 28), "hatch"))

    # Lamps, displays, mirrors on arms.
    add(Box("destination", (-17, 42, -B_HL), (17, 46, -B_HL + 0.5), "dest", lamp="glow"))
    add(Box("route_rear", (-4, 43, B_HL), (4, 47, B_HL + 0.25), "route", lamp="glow"))
    boxes += pair(Box("headlight_right", (11.0, 9, -B_HL - 0.25), (E, 13, -B_HL), "headlight", lamp="head"))
    boxes += pair(Box("taillight_right", (13.0, 8, B_HL), (E, 20, B_HL + 0.25), "taillight", lamp="brake"))
    add(Box("plate_front", (-4.5, 4, -B_HL - 0.25), (4.5, 6, -B_HL), "plate_front"))
    add(Box("plate_rear", (-4.5, 4, B_HL), (4.5, 6, B_HL + 0.25), "plate_rear"))
    boxes += pair(Box("mirror_arm_right", (W - 1, 42, -77), (W + 2.5, 43, -76), "trim"))
    boxes += pair(Box("mirror_right", (W + 1.5, 33, -77.5), (W + 3, 42, -76), "mirror"))

    bones = {}
    disc = [(8, 3, 0.5), (7, 5, 0.25), (5, 7, 0.25), (3, 8, 0.5), (6, 6, 0.0)]
    boxes += wheel_boxes(bones, "front", (16.0, B_WR, -B_WZ), 13.0, 19.0, disc)
    rear = wheel_boxes(bones, "rear", (14.5, B_WR, B_WZ), 10.0, 19.0, disc)
    for b in rear:
        b.extra["twin"] = 14
    boxes += rear

    painters = {
        "body": paint_bus_body,
        "under": lambda c: UNDER,
        "pod": paint_bus_pod,
        "hatch": lambda c: PLASTIC_HI if c.top else PLASTIC,
        "dest": paint_bus_display(DEST),
        "route": paint_bus_display(ROUTE),
        "headlight": paint_bus_headlight,
        "taillight": paint_bus_tail,
        "plate_front": plate(False),
        "plate_rear": plate(True),
        "trim": lambda c: UNDER if c.bottom else TRIM,
        "mirror": mirror_painter(W + 3),
        "wheel": paint_wheel_truck,
    }
    return Model(
        "car_bus", boxes, painters, {"body": "minecraft:yellow_concrete"},
        bones=bones, eye=(0.0, 2.0 * M, -3.9 * M),
    )


MODELS = [build_van, build_pickup, build_truck, build_bus]
