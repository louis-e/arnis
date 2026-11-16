"""Traffic signal heads: a three-lamp head for cars (bikes get it smaller) and a two-lamp
walker's head, one model per state the data pack switches between.

Signals are small, so these are drawn at 32 units to the metre, twice the vehicles'
density, for round lenses and a readable little man; Arnis draws them at half the size.
Front (the lenses) towards -z, the back of the housing, where it meets the pole, at z = 0,
centred on x, standing on y = 0.
"""
import math

from kit import Box, Model, hexc, mix

HOUSING = hexc("#26282b")
HOUSING_EDGE = hexc("#3a3d41")
VISOR = hexc("#17181a")
PLATE = hexc("#121314")
PLATE_BORDER = hexc("#e9e9e4")
BRACKET = hexc("#8a8c8a")

# Lit and dark lens colours: (edge, centre).
LENSES = {
    "red": ((hexc("#d81e14"), hexc("#ff5a3c")), (hexc("#3a0b08"), hexc("#55130e"))),
    "amber": ((hexc("#f08c00"), hexc("#ffc23a")), (hexc("#3d2604"), hexc("#5a3a08"))),
    "green": ((hexc("#00c878"), hexc("#5affb4")), (hexc("#06331f"), hexc("#0b4a2e"))),
}

# The car head: three lenses 8 units across, 11 apart, under hoods.
CAR_W, CAR_H, CAR_D = 5.5, 35.0, 8.0
CAR_LAMPS = (("red", 29.0), ("amber", 18.0), ("green", 7.0))
LENS_R = 4.0
VISOR_D = 4.5

# The walker's head: two square lenses with the little man.
WALK_W, WALK_H, WALK_D = 7.0, 25.0, 8.0
WALK_LAMPS = (("red", 18.0), ("green", 7.0))
WALK_R = 5.0

STANDING = [
    "....##....",
    "....##....",
    "...####...",
    "..######..",
    "..#.##.#..",
    "..#.##.#..",
    "....##....",
    "...#..#...",
    "...#..#...",
    "...#..#...",
]
WALKING = [
    ".....##...",
    ".....##...",
    "....###...",
    "...#####..",
    "..#.###.#.",
    "....###...",
    "....#.#...",
    "...#...#..",
    "..#.....#.",
    ".#.......#",
]


def housing_painter(c):
    if c.front or c.back:
        return HOUSING
    if c.top:
        return HOUSING_EDGE
    # A lighter line down the front edges, as on a rounded housing.
    if c.side and c.p[2] < -CAR_D + 1:
        return HOUSING_EDGE
    return mix(HOUSING, (0, 0, 0), 0.25) if c.bottom else HOUSING


def plate_painter(c):
    if not c.front:
        return PLATE
    x, y = abs(c.p[0]), c.p[1]
    if x > 8.0 or y < 1.0 or y > 36.0:
        return PLATE_BORDER
    return PLATE


def lens_painter(colour, lit, centre, radius, picture=None):
    (edge, mid), (dark_edge, dark_mid) = LENSES[colour]

    def paint(c):
        if not c.front:
            return HOUSING
        dx, dy = c.p[0], c.p[1] - centre
        if picture is not None:
            # A square lens: the little man lit, or faintly there when dark.
            n = len(picture)
            i = int((dx + radius) / (2 * radius) * n)
            j = int((centre + radius - c.p[1]) / (2 * radius) * n)
            on = 0 <= i < n and 0 <= j < n and picture[j][i] == "#"
            if on:
                return mid if lit else dark_mid
            return hexc("#0c0c0d")
        r = math.hypot(dx, dy)
        if r > radius:
            return HOUSING
        if lit:
            # Brightest in the middle, a white glint up and to the left.
            if -2.5 < dx < -1.0 and 1.0 < dy < 2.5:
                return mix(mid, (255, 255, 255), 0.55)
            return mix(mid, edge, min(1.0, r / radius) ** 2)
        if -2.5 < dx < -1.0 and 1.0 < dy < 2.5:
            return mix(dark_mid, (180, 190, 200), 0.25)
        return mix(dark_mid, dark_edge, min(1.0, r / radius))

    return paint


def visor_painter(c):
    if c.bottom:
        return mix(VISOR, (0, 0, 0), 0.4)
    return VISOR


def head(boxes, painters, lamps, w, h, d, radius, lit, pictures=None):
    """The housing, a lens and a hood per lamp, and the brackets onto the pole."""
    boxes.append(Box("housing", (-w, 0, -d), (w, h, 0), "housing"))
    painters["housing"] = housing_painter
    painters["visor"] = visor_painter
    for colour, y in lamps:
        mat = f"lens_{colour}"
        on = colour in lit
        picture = pictures.get(colour) if pictures else None
        boxes.append(Box(
            mat, (-radius, y - radius, -d - 0.25), (radius, y + radius, -d), mat,
            lamp="glow" if on else None,
        ))
        painters[mat] = lens_painter(colour, on, y, radius, picture)
        # A hood over the lens, open below, its cheeks short of the lens's middle.
        top = y + radius + 0.5
        boxes.append(Box(f"visor_{colour}", (-radius - 1, top, -d - VISOR_D), (radius + 1, top + 1, -d), "visor"))
        for side in (-1, 1):
            x0 = radius if side > 0 else -radius - 1
            boxes.append(Box(
                f"cheek_{colour}_{'r' if side > 0 else 'l'}",
                (x0, y + 0.5, -d - VISOR_D * 0.6), (x0 + 1, top, -d), "visor",
            ))
    for y in (2.0, h - 4.0):
        boxes.append(Box(f"bracket_{int(y)}", (-1.5, y, 0), (1.5, y + 2, 1.5), "bracket"))
    painters["bracket"] = lambda c: BRACKET


def car_head(state):
    lit = {"red": ("red",), "amber": ("amber",), "green": ("green",), "red_amber": ("red", "amber")}[state]
    boxes, painters = [], {}
    head(boxes, painters, CAR_LAMPS, CAR_W, CAR_H, CAR_D, LENS_R, lit)
    # The backplate, black with a white border, round the housing at its middle.
    boxes.append(Box("plate", (-9, -1.5, -4.5), (9, 37.5, -3.5), "plate"))
    painters["plate"] = plate_painter
    return Model(f"signal_car_{state}", boxes, painters, {"body": "minecraft:black_concrete"})


def walk_head(state):
    boxes, painters = [], {}
    head(boxes, painters, WALK_LAMPS, WALK_W, WALK_H, WALK_D, WALK_R, (state,),
         pictures={"red": STANDING, "green": WALKING})
    return Model(f"signal_walk_{state}", boxes, painters, {"body": "minecraft:black_concrete"})


MODELS = [
    lambda: [car_head(s) for s in ("red", "amber", "green", "red_amber")],
    lambda: [walk_head(s) for s in ("red", "green")],
]
