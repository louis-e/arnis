"""Level crossing gear: the warning mast beside the road, a model per state the data pack
switches between (dark, and each of its two red lamps lit in turn), and the barrier boom
the pack lowers across the lanes.

Like the signal heads these are small, so they are drawn at 32 units to the metre and
Arnis draws them at half the size. The mast stands on y = 0, centred on x and z, its lamps
towards -z, where the traffic it stops comes from; the road is on its +x side, and so is
the drive that carries the boom. The boom is a model of its own, lying along +x from its
hinge at the origin, which Java's model is fitted about: the pack swings it up by turning
its display, and Arnis stretches it along x to the width of the lanes it closes.
"""
import math

from kit import Box, Model, hexc, mix

CONCRETE = hexc("#9a9c98")
CONCRETE_DARK = hexc("#7d7f7c")
MAST = hexc("#b9bdc1")
MAST_SHADE = hexc("#8d9197")
DRIVE = hexc("#c9ccc8")
DRIVE_SHADE = hexc("#9fa3a0")
DRIVE_DOOR = hexc("#b4b8b4")
HUB = hexc("#3b3e43")
HUB_HI = hexc("#565a60")
BOARD = hexc("#f1f1ec")
BOARD_RED = hexc("#c8201c")
BOARD_BACK = hexc("#a9adb1")
TARGET = hexc("#141516")
TARGET_EDGE = hexc("#2c2e31")
HOOD = hexc("#18191b")
BOOM_WHITE = hexc("#eeeeea")
BOOM_RED = hexc("#d0241f")
BOOM_SHADE = 0.14
REFLECTOR = hexc("#ff5a48")
WEIGHT = hexc("#4a4d52")
WEIGHT_HI = hexc("#6a6e74")

# Lit and dark lens colours: (edge, centre).
LIT = (hexc("#d81e14"), hexc("#ff5a3c"))
DARK = (hexc("#3a0b08"), hexc("#55130e"))

# The mast: its lamps 0.75 m apart at 2.3 m, the cross above them.
MAST_TOP = 106.0
LAMP_Y = 74.0
LAMP_X = 12.0
LENS_R = 4.5
TARGET_R = 8.0
CROSS_Y = 98.0
CROSS_LEN = 19.0
CROSS_W = 3.0
# The boom's hinge, on the front of the drive: 1 m up, in line with the mast so the boom
# can lie to either side, and far enough in front of it to stand clear of the lamp hoods.
# level_crossings.rs puts the boom's display there.
HINGE = (0.0, 32.0, -15.0)
# The boom: 4 m from its hinge, striped every half metre, a counterweight behind.
BOOM_LEN = 128.0
BOOM_H = 2.5
BOOM_D = 1.5
STRIPE = 16.0
WEIGHT_LEN = 24.0


def concrete(c):
    if c.top:
        return CONCRETE
    return CONCRETE_DARK if c.bottom or c.p[1] < 1.0 else mix(CONCRETE, CONCRETE_DARK, 0.4)


def mast(c):
    if c.top:
        return MAST
    # Lit from the front left, as everything in the kit is.
    return MAST if c.front or c.p[0] < 0 else MAST_SHADE


def drive(c):
    if c.top:
        return DRIVE
    if c.bottom:
        return DRIVE_SHADE
    x, y = c.p[0], c.p[1]
    if c.back:
        # A door with a frame and a handle, on the side away from the boom.
        if -5.0 < x < 5.0 and 19.0 < y < 43.0:
            if 2.5 < x < 3.5 and 29.0 < y < 33.0:
                return HUB
            return DRIVE_DOOR
        return DRIVE_SHADE
    if c.side:
        return DRIVE_SHADE if y < 18.0 else mix(DRIVE, DRIVE_SHADE, 0.5)
    return DRIVE


def hub(c):
    if c.front or c.back:
        return HUB_HI if math.hypot(c.loc[0], c.loc[1]) < 1.5 else HUB
    return HUB


def board(c):
    """One arm of the cross, painted along its own length: white, its ends red."""
    if not (c.front or c.back):
        return BOARD_BACK
    along = abs(c.loc[0])
    if along > CROSS_LEN - 6.0:
        return BOARD_RED
    return BOARD if c.front else mix(BOARD, BOARD_BACK, 0.5)


def target(c):
    """The black disc behind a lamp."""
    if not (c.front or c.back):
        return TARGET_EDGE
    dx, dy = abs(c.p[0]) - LAMP_X, c.p[1] - LAMP_Y
    return TARGET_EDGE if math.hypot(dx, dy) > TARGET_R - 1.0 else TARGET


def lens(lit):
    edge, mid = LIT if lit else DARK

    def paint(c):
        if not (c.front or c.back):
            return HOOD
        dx, dy = abs(c.p[0]) - LAMP_X, c.p[1] - LAMP_Y
        r = math.hypot(dx, dy)
        if r > LENS_R:
            return TARGET
        glint = -2.5 < dx < -1.0 and 1.0 < dy < 2.5
        if lit:
            return mix(mid, (255, 255, 255), 0.55) if glint else mix(mid, edge, min(1.0, r / LENS_R) ** 2)
        return mix(mid, (180, 190, 200), 0.25) if glint else mix(mid, edge, min(1.0, r / LENS_R))

    return paint


def hood(c):
    return mix(HOOD, (0, 0, 0), 0.4) if c.bottom else HOOD


def light(state):
    """The mast with `state` lit: "off", or "a" or "b" for the lamp on its -x or +x side,
    seen from the front, and the one behind it on the other side of the mast."""
    boxes = [
        Box("foundation", (-6, 0, -6), (6, 5, 6), "concrete"),
        Box("mast", (-2, 5, -2), (2, MAST_TOP, 2), "mast"),
        # The barrier drive, in front of the mast.
        Box("drive", (-7, 14, HINGE[2] + 1.5), (7, 46, -2), "drive"),
        Box("drive_foot", (-5, 5, -11), (5, 14, -4), "mast"),
        Box("arm", (-LAMP_X, LAMP_Y - 1.5, -1.5), (LAMP_X, LAMP_Y + 1.5, 1.5), "mast"),
    ]
    painters = {"concrete": concrete, "mast": mast, "drive": drive, "board": board,
                "target": target, "hood": hood}
    for turn in (45.0, -45.0):
        name = "cross_a" if turn > 0 else "cross_b"
        # Two boards a hair apart, so the one over the other does not flicker.
        z = -3.5 if turn > 0 else -3.75
        boxes.append(Box(
            name, (-CROSS_LEN, CROSS_Y - CROSS_W, z), (CROSS_LEN, CROSS_Y + CROSS_W, z + 1.0), "board",
            origin=(0, CROSS_Y, z), rot=(0, 0, turn),
        ))
    for side, x in (("a", -LAMP_X), ("b", LAMP_X)):
        for face, z0, z1, zl in (("front", -3.0, -1.5, -3.25), ("back", 1.5, 3.0, 3.0)):
            # The lamp behind lights with the one in front of the other side, so both
            # sides of the mast show one lamp at a time.
            on = state == side if face == "front" else state not in ("off", side)
            mat = "lit" if on else "dark"
            painters[mat] = lens(on)
            boxes.append(Box(f"target_{side}_{face}", (x - TARGET_R, LAMP_Y - TARGET_R, z0),
                             (x + TARGET_R, LAMP_Y + TARGET_R, z1), "target"))
            boxes.append(Box(f"lens_{side}_{face}", (x - LENS_R, LAMP_Y - LENS_R, zl),
                             (x + LENS_R, LAMP_Y + LENS_R, zl + 0.25), mat,
                             lamp="glow" if on else None))
            h0, h1 = (z0 - 6.0, z0) if face == "front" else (z1, z1 + 6.0)
            boxes.append(Box(f"hood_{side}_{face}", (x - LENS_R - 1, LAMP_Y + LENS_R + 0.5, h0),
                             (x + LENS_R + 1, LAMP_Y + LENS_R + 1.5, h1), "hood"))
    return Model(f"xing_light_{state}", boxes, painters, {"body": "minecraft:black_concrete"})


def stripes(c):
    """Red and white along the boom, the tip red, darker underneath."""
    x = c.p[0]
    red = int(x // STRIPE) % 2 == 1 or x > BOOM_LEN - STRIPE / 2
    base = BOOM_RED if red else BOOM_WHITE
    if c.bottom:
        return mix(base, (0, 0, 0), 2 * BOOM_SHADE)
    if c.top:
        return mix(base, (255, 255, 255), BOOM_SHADE)
    # A reflector in the middle of each red stripe.
    if red and (c.front or c.back) and abs(x % STRIPE - STRIPE / 2) < 1.5 and abs(c.p[1]) < 1.0:
        return REFLECTOR
    return base


def weight(c):
    if c.top:
        return WEIGHT_HI
    return mix(WEIGHT, (0, 0, 0), 0.3) if c.bottom else WEIGHT


def boom():
    boxes = [
        Box("arm", (0, -BOOM_H, -BOOM_D), (BOOM_LEN, BOOM_H, BOOM_D), "stripes"),
        Box("tail", (-WEIGHT_LEN, -BOOM_H - 1, -BOOM_D - 0.5), (0, BOOM_H + 1, BOOM_D + 0.5), "weight"),
        Box("weights", (-WEIGHT_LEN - 0.5, -BOOM_H - 4, -BOOM_D - 1.5), (-WEIGHT_LEN + 9, BOOM_H + 1.5, BOOM_D + 1.5), "weight"),
        Box("hub", (-4, -4, -BOOM_D - 1.0), (4, 4, BOOM_D + 1.0), "hub"),
    ]
    painters = {"stripes": stripes, "weight": weight, "hub": hub}
    return Model("xing_boom", boxes, painters, {"body": "minecraft:black_concrete"}, centre=(0, 0, 0))


MODELS = [
    lambda: [light(s) for s in ("off", "a", "b")],
    boom,
]
