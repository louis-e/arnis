"""Small boats and river craft: motorboat, sailboat, river ferry, water bus, tour boat and
peniche, the last two also in a low build for low bridges.

Sizes, heights and seats follow ship_bodies.rs. The waterline is y 0, the bow at -z. Hull
in the livery's "lower", the sheer stripe and boot-top in "stripe", superstructure in
"body", decks in "door", roofs in "roof".
"""
import math

from kit import (
    CHROME, CHROME_DARK, GLASS_DARK, GLASS_HI, GLASS_LIGHT, GLASS_MID, STEEL, TRIM, UNDER,
    Box, Model, Paint, glass, hexc, mix, pair, wheel_face,
)

# ---------------------------------------------------------------- colours

ANTIFOUL = hexc("#5c2421")
ANTIFOUL_DARK = hexc("#45191a")
RUB = hexc("#d6dade")
FENDER = hexc("#222326")
FENDER_HI = hexc("#3a3c41")
SAIL = hexc("#efede6")
SAIL_SHADE = hexc("#d9d6cc")
SAIL_SEAM = hexc("#c9c5b9")
MAST = hexc("#c3c8ce")
MAST_DARK = hexc("#8f959d")
CUSHION = hexc("#e8e2d2")
CUSHION_SHADE = hexc("#cfc7b3")
CUSHION_PIPE = hexc("#b9b09a")
RAIL = hexc("#dde1e5")
RAIL_DARK = hexc("#a9afb6")
ENGINE = hexc("#26282d")
ENGINE_HI = hexc("#3b3e45")
NAV_RED = hexc("#ff3a2e")
NAV_GREEN = hexc("#36e070")
NAV_WHITE = hexc("#fff6dc")
DASH = hexc("#2a2b30")
DIAL = hexc("#cfd6de")


# ---------------------------------------------------------------- building helpers


def zflip(box, name=None):
    """The same box mirrored front to back (z -> -z)."""
    return Box(
        name or box.name,
        (box.frm[0], box.frm[1], -box.to[2]),
        (box.to[0], box.to[1], -box.frm[2]),
        box.mat, box.bone,
        (box.origin[0], box.origin[1], -box.origin[2]),
        (-box.rot[0], -box.rot[1], box.rot[2]),
        box.lamp, **box.extra,
    )


def run(box, n, dz, name=None):
    """n copies of a box, dz apart along the boat: segments whose faces come out alike."""
    base = name or box.name
    return [box.moved(dz=k * dz, name=f"{base}_{k}") for k in range(n)]


class VEnd:
    """A pointed end in plan view: two slabs turned about y meet at a narrow stem post, and
    level steps fill the space between them, each short enough that its corners stay inside
    the slabs. Built for a bow at -z; `zflip` turns it into a stern."""

    def __init__(self, tip, join, half, stem, thick):
        self.tip, self.join, self.half, self.stem, self.thick = tip, join, half, stem, thick
        self.angle = math.degrees(math.atan2(half - stem, join - tip))
        assert self.angle <= 45.0, self.angle
        self.tan = math.tan(math.radians(self.angle))
        self.length = math.hypot(half - stem, join - tip)
        self.inset = thick / math.cos(math.radians(self.angle))

    def x_out(self, z):
        return self.stem + (z - self.tip) * self.tan

    def slab(self, name, y0, y1, mat, **extra):
        """The right-hand slab; pair() it."""
        return Box(
            name, (self.stem - self.thick, y0, self.tip), (self.stem, y1, self.tip + self.length), mat,
            origin=(self.stem, y0, self.tip), rot=(0, self.angle, 0), **extra,
        )

    def along(self, name, inset, width, y0, y1, d0, d1, mat, **extra):
        """A right-hand box in the slab's own frame: `inset` in from its outer face, from
        `d0` to `d1` along it from the tip. For rails and posts that follow a turned side."""
        return Box(
            name, (self.stem - inset - width, y0, self.tip + d0), (self.stem - inset, y1, self.tip + d1), mat,
            origin=(self.stem, y0, self.tip), rot=(0, self.angle, 0), **extra,
        )

    def post(self, name, y0, y1, mat, depth=None, **extra):
        depth = depth or min(self.thick / math.sin(math.radians(self.angle)) * 0.9, 6.0, self.join - self.tip)
        return Box(name, (-self.stem, y0, self.tip), (self.stem, y1, self.tip + depth), mat, **extra)

    def fill(self, name, y0, y1, mat, start, **extra):
        """Level steps from z `start` (behind the post) to the join."""
        most = self.thick / math.sin(math.radians(self.angle)) * 0.92
        n = max(1, math.ceil((self.join - start) / most))
        step = (self.join - start) / n
        out = []
        for k in range(n):
            z, z1 = start + k * step, start + (k + 1) * step
            w = self.x_out(z) - 0.08
            assert w >= self.x_out(z1) - self.inset, (name, z, z1)
            out.append(Box(f"{name}_{k}", (-w, y0, z), (w, y1, z1), mat, **extra))
        return out


LIFT = 0.3


def layered_end(v, name, layers, mat, fill_mat, deck=None, plate=False, **extra):
    """A raked stem: `layers` are (y0, y1, setback), each layer's tip `setback` behind the
    master end `v`'s tip along the same slab line. The steps of the top layer stop at `deck`
    so its slabs stand up as a bulwark; right-hand slabs are paired.

    No two faces of it lie in one plane: the lowest slabs stand LIFT above the bottom of
    their post and steps, a narrow post reaches LIFT past its slabs above and below (it
    covers where the two slabs cross at the stem), and the steps of the upper layers reach
    LIFT down into the layer under them."""
    out = []
    for k, (y0, y1, back) in enumerate(layers):
        e = VEnd(tip=v.tip + back, join=v.join, half=v.half, stem=v.x_out(v.tip + back), thick=v.thick)
        top = deck if (deck is not None and k == len(layers) - 1) else y1
        right = e.slab(f"{name}{k}_right", y0 + (LIFT if k == 0 else 0), y1, mat, **extra)
        if top < y1 and not plate:
            # The two bulwark tops cross at a narrow stem: keep them out of one plane by a
            # few hundredths of a metre, which no one sees.
            out += [right, e.slab(f"{name}{k}_right", right.frm[1], y1 - LIFT, mat, **extra).mirrored()]
        else:
            out += pair(right)
        low = y0 if k == 0 else y0 - LIFT
        post = e.post(f"{name}{k}_stem", low, top if (plate and top < y1) else y1 + LIFT, mat, **extra)
        out.append(post)
        if plate and top < y1:
            # A wide stem gets its bulwark across the front, a quarter lower than the slabs
            # so their tops never lie in one plane.
            out.append(Box(f"{name}{k}_plate", (-e.stem, top, e.tip), (e.stem, y1 - 0.25, e.tip + 1.5), mat, **extra))
        if post.to[2] < e.join - 0.5:
            out += e.fill(f"{name}{k}_fill", low, top, fill_mat, post.to[2], **extra)
    return out


def rows(y):
    return math.floor(y + 1e-6)


def inward(c):
    """Whether an upright face looks in towards the middle of the boat (a bulwark's or a
    cockpit's inside) rather than out over the water. Works on turned faces too."""
    return abs(c.n[1]) < 0.5 and c.n[0] * c.p[0] + c.n[2] * c.p[2] < 0


def planks(x, z, width=3, length=None, k=0.0, slot="door"):
    """Deck planks along the boat with dark seams, and staggered butts every `length`."""
    ix = math.floor(x + 64)
    col = ix // width
    if ix % width == 0:
        return Paint(slot, k - 0.3)
    if length:
        iz = math.floor(z + 1024)
        if (iz + col * 17) % length == 0:
            return Paint(slot, k - 0.22)
    return Paint(slot, k + (0.04 if col % 3 == 0 else -0.03 if col % 3 == 1 else 0.0))


def hull_bands(y, sheer, stripe, boot=(0, 2), slot="lower", stripe_slot="stripe", rub=True):
    """A hull side: antifouling under the waterline, boot-top, topsides shaded towards the
    sheer, a sheer stripe and a rub rail along the top."""
    iy = rows(y)
    if iy < boot[0]:
        return ANTIFOUL_DARK if iy < boot[0] - 3 else ANTIFOUL
    if iy < boot[1]:
        return Paint(stripe_slot, -0.06 if iy == boot[0] else 0.04)
    if rub and iy == sheer - 1:
        return RUB
    if rub and iy == sheer - 2:
        return Paint(slot, -0.3)
    if stripe[0] <= iy < stripe[1]:
        return Paint(stripe_slot, 0.06 if iy == stripe[1] - 1 else -0.04)
    if iy == stripe[1]:
        return Paint(slot, 0.08)
    if iy == boot[1]:
        return Paint(slot, -0.16)
    return Paint(slot, -0.1 + 0.1 * min(1.0, (iy - boot[1]) / max(1, stripe[0] - boot[1])))


def nav_side(c):
    """A sidelight: red to port (-x), green to starboard (+x)."""
    return NAV_RED if c.p[0] < 0 else NAV_GREEN


# ---------------------------------------------------------------- motorboat

MB_HALF = 2.6 * 8          # 20.8
MB_BOW = VEnd(tip=-56.0, join=-12.0, half=MB_HALF, stem=1.5, thick=3.0)
MB_SHEER, MB_BOW_SHEER, MB_DECK, MB_SOLE = 14, 15, 14, 7
MB_STRIPE = (8, 10)


def mb_hull(c):
    x, y, z = c.p
    sheer = c.box.extra.get("sheer", MB_SHEER)
    if c.bottom:
        return ANTIFOUL_DARK
    if c.top:
        if y < MB_SOLE + 0.5:
            return planks(x, z)
        return Paint("body", 0.14)
    if inward(c):
        return Paint("body", -0.08 if rows(y) < sheer - 1 else 0.02)
    return hull_bands(y, sheer, MB_STRIPE)


def mb_deck(c):
    x, y, z = c.p
    ix, iz = math.floor(x), math.floor(z)
    if c.top:
        if y < MB_SOLE + 0.5:
            return planks(x, z)
        # The foredeck: white, a hatch and a cleat line.
        if -38 <= iz < -28 and -5 <= ix < 5:
            if iz in (-38, -29) or ix in (-5, 4):
                return TRIM
            return glass(20, ix, iz)
        if ix in (-1, 0) and iz < -40:
            return Paint("body", -0.06)
        return Paint("body", 0.1)
    if c.bottom:
        return ANTIFOUL_DARK
    if c.back:
        # The dash under the windscreen, the helm on the right.
        iy = rows(y)
        if iy >= MB_DECK - 1:
            return Paint("body", 0.06)
        if 9 <= iy <= 13 and 3 <= ix <= 15:
            if iy in (11, 12) and ix in (5, 6, 8, 9):
                return DIAL
            return DASH
        if 10 <= iy <= 12 and -12 <= ix <= -4:
            return Paint("body", -0.2) if iy == 10 else Paint("body", -0.12)
        return Paint("body", -0.06)
    return hull_bands(y, c.box.extra.get("sheer", MB_BOW_SHEER), MB_STRIPE)


def mb_windscreen(c):
    x, y, z = c.p
    lx, ly = c.loc[0], c.loc[1] - MB_DECK
    if c.side and abs(lx) > 15.9:
        lz = c.loc[2] + 18.5
        if ly > 8.6:
            return CHROME
        if lz > 4.6 or lz < 0.6:
            return CHROME_DARK
        return glass(ly, lz, ly, top=8.0, bottom=1.0)
    if c.face in ("north", "south"):
        if ly > 8.6 or abs(lx) > 15.4:
            return CHROME if ly > 8.6 else CHROME_DARK
        if ly < 1.0:
            return TRIM
        return glass(ly, lx, ly, top=8.0, bottom=1.0)
    if c.top or c.side:
        return CHROME
    return TRIM


def mb_seat(c):
    x, y, z = c.p
    if c.bottom:
        return UNDER
    iy = rows(y)
    if c.top:
        if math.floor(abs(x)) in (8,) or math.floor(x) == 0:
            return CUSHION_PIPE
        return CUSHION
    if iy == math.floor(c.box.frm[1]):
        return CUSHION_SHADE
    if iy == math.ceil(c.box.to[1]) - 1:
        return CUSHION
    return mix(CUSHION, CUSHION_SHADE, 0.5)


def mb_engine(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top:
        return ENGINE_HI if abs(x) < 3 and 57 <= z < 62.5 else ENGINE
    if c.bottom:
        return ENGINE
    if iy <= 12:
        return CHROME_DARK if iy == 12 else STEEL
    if iy == 13:
        return ENGINE
    if iy == 17 and (c.side or c.back):
        return CHROME
    if iy == 20:
        return ENGINE_HI
    return ENGINE


def mb_leg(c):
    x, y, z = c.p
    if rows(y) < -6:
        return ENGINE
    return mix(ENGINE, STEEL, 0.25) if c.side else ENGINE


def build_motorboat():
    boxes = []
    add = boxes.append
    v = MB_BOW
    hw = MB_HALF
    # Hull: the bow slabs from the stem to the windscreen, the steps between them carry the
    # foredeck; aft of that the cockpit tub, its sides and the transom.
    # Hull: the bow in two layers, the lower one shorter, for a raked stem; the steps between
    # the slabs carry the foredeck. Aft of that the cockpit tub, its sides and the transom.
    boxes += layered_end(v, "bow", [(-1, 2, 9.0), (2, MB_SOLE, 4.5), (MB_SOLE, MB_BOW_SHEER, 0.0)], "hull", "deck",
                         deck=MB_DECK, sheer=MB_BOW_SHEER)
    add(Box("tub", (-hw, -1, v.join), (hw, MB_SOLE, 56), "hull", sheer=MB_SHEER))
    boxes += pair(Box("side_right", (hw - 3, MB_SOLE, v.join), (hw, MB_SHEER, 56), "hull", sheer=MB_SHEER))
    boxes += pair(Box("transom_right", (5, MB_SOLE, 52), (hw - 3, MB_SHEER, 56), "hull", sheer=MB_SHEER))
    add(Box("motor_well", (-5, MB_SOLE, 52), (5, 10, 56), "hull", sheer=10))
    # Under water: a stepped V and keel, ending short of the bow.
    add(Box("vee", (-15, -4, -24), (15, -1, 56), "hull"))
    add(Box("keel", (-7, -7, -12), (7, -4, 54), "hull"))
    # The windscreen, leaned back over the dash.
    add(Box("windscreen", (-16, MB_DECK, -18.5), (16, MB_DECK + 9.5, -17.5), "windscreen",
            origin=(0, MB_DECK, -18.5), rot=(32, 0, 0)))
    boxes += pair(Box("windscreen_wing_right", (16, MB_DECK, -18.5), (17, MB_DECK + 9.5, -13.0), "windscreen",
                      origin=(0, MB_DECK, -18.5), rot=(32, 0, 0)))
    add(Box("wheel", (8, 9.5, -12), (12, 13.5, -11), "trim"))
    # Seats: two buckets forward, a bench aft against the sun pad over the engine well.
    for name, x0 in (("helm_seat", 4.5), ("mate_seat", -13.5)):
        add(Box(name, (x0, MB_SOLE, -2), (x0 + 9, 11, 5), "seat"))
        add(Box(name + "_back", (x0, 11, 4), (x0 + 9, 19, 6), "seat"))
    add(Box("bench", (-17.8, MB_SOLE, 28), (17.8, 11, 38), "seat"))
    add(Box("bench_back", (-17.8, 11, 38), (17.8, 18, 41), "seat"))
    add(Box("sun_pad", (-17.8, MB_SOLE, 41), (17.8, 13.5, 52), "seat"))
    # Outboard on the transom.
    add(Box("bracket", (-3, 5, 56), (3, 11, 57.5), "leg"))
    add(Box("cowling", (-4, 11, 55.5), (4, 21, 63.5), "engine"))
    add(Box("leg", (-1.5, -7, 57.5), (1.5, 11, 61.5), "leg"))
    add(Box("plate", (-3.5, -3, 56.5), (3.5, -2, 63), "leg"))
    add(Box("prop", (-3, -8, 62), (3, -4, 63), "leg"))
    # Lights: red and green at the stem, a white all-round light on a pole aft.
    add(Box("bow_light", (-1.2, MB_BOW_SHEER + LIFT, -55.5), (1.2, MB_BOW_SHEER + LIFT + 1.5, -52.5), "nav", lamp="glow"))
    add(Box("stern_pole", (14.5, MB_SHEER, 52.5), (15.5, 27, 53.5), "rail"))
    add(Box("stern_light", (14, 27, 52), (16, 29, 54), "white_light", lamp="glow"))
    # Cleats.
    add(Box("cleat_bow", (-1, MB_DECK, -47), (1, MB_DECK + 1, -44), "rail"))
    boxes += pair(Box("cleat_aft_right", (hw - 2.5, MB_SHEER, 48), (hw - 1.5, MB_SHEER + 1, 51), "rail"))

    painters = {
        "hull": mb_hull,
        "deck": mb_deck,
        "windscreen": mb_windscreen,
        "trim": lambda c: TRIM,
        "seat": mb_seat,
        "engine": mb_engine,
        "leg": mb_leg,
        "nav": nav_side,
        "rail": lambda c: RAIL if c.top else RAIL_DARK,
        "white_light": lambda c: NAV_WHITE,
    }
    # Seat from ship_bodies.rs: 1.9 m aft of the middle, eyes 1.7 m over the water.
    return Model("ship_motorboat", boxes, painters,
                 {"body": "minecraft:white_concrete", "lower": "minecraft:white_concrete",
                  "stripe": "minecraft:blue_concrete", "door": "minecraft:light_gray_terracotta",
                  "roof": "minecraft:white_concrete"},
                 eye=(0.0, 1.7 * 16, 1.9 * 16))



# ---------------------------------------------------------------- sailboat

SB_HALF = 3.4 * 8          # 27.2
SB_BOW = VEnd(tip=-80.0, join=-26.0, half=SB_HALF, stem=1.5, thick=3.0)
SB_STERN = VEnd(tip=-80.0, join=-26.0, half=SB_HALF, stem=22.0, thick=7.0)
SB_DECK, SB_SOLE, SB_SHEER, SB_BOW_SHEER = 13, 8, 14, 15
SB_STRIPE = (9, 11)
SB_CABIN = (-20.0, 26.0)   # the trunk, behind its raked front
SB_TRUNK_TOP = 22
MAST_Z = (-23.0, -20.5)
WOOD = hexc("#8a5a35")
WOOD_DARK = hexc("#6a4327")


def sb_hull(c):
    x, y, z = c.p
    sheer = c.box.extra.get("sheer", SB_SHEER)
    if c.bottom:
        return ANTIFOUL_DARK
    if c.top:
        if y < SB_SOLE + 0.5:
            return planks(x, z)
        if y < SB_DECK + 0.5:
            return planks(x, z)
        # A side deck or toe rail: teak, with the rail's white cap along the outer edge.
        if c.box.rot[1] and c.box.extra.get("deckwide"):
            outer = (c.box.to[0] - c.loc[0]) if x > 0 else (c.loc[0] - c.box.frm[0])
            if outer < 1.0:
                return RUB
            return planks(c.loc[0], c.loc[2])
        return RUB
    if inward(c):
        iy = rows(y)
        if iy >= sheer - 1:
            return RUB
        return Paint("body", -0.04 if iy >= SB_SOLE + 4 else -0.12)
    return hull_bands(y, sheer, SB_STRIPE)


def sb_trunk(c):
    x, y, z = c.p
    iy, iz = rows(y), math.floor(z)
    if c.top:
        # Cabin top: hatches and teak handrails.
        ax = abs(x)
        if -16 <= iz < -6 and ax < 6:
            return TRIM if (iz in (-16, -7) or ax > 5) else glass(20, x, z)
        if 10 <= iz < 26 and ax < 6:
            return Paint("roof", -0.18) if ax > 5 or iz == 10 else Paint("roof", 0.0) if iz % 3 else Paint("roof", -0.08)
        if math.floor(ax) == 13 and -14 <= iz < 20:
            return WOOD
        return Paint("roof", 0.06)
    if c.bottom:
        return UNDER
    if c.side:
        if iy == SB_TRUNK_TOP - 1:
            return Paint("body", 0.1)
        # Long portlights with rounded ends.
        if 16 <= iy <= 18 and -12 <= iz < 18:
            if (iz in (-12, 17) and iy in (16, 18)):
                return Paint("body", -0.1)
            return glass(y, z, y, top=19, bottom=16)
        return Paint("body", -0.02 if iy > 14 else -0.1)
    if c.back:
        # The companionway, hatch boards in teak.
        if abs(x) < 5 and iy >= SB_SOLE:
            if abs(x) > 4:
                return Paint("body", -0.2)
            return WOOD_DARK if iy % 3 == 0 else WOOD
        return Paint("body", -0.06)
    return Paint("body", 0.0)


def sb_front(c):
    x, y, z = c.p
    if c.face == "north":
        iy = rows(c.loc[1] - SB_DECK)
        if 4 <= iy <= 6 and abs(x) < 13:
            return glass(iy, x, iy, top=7, bottom=4) if abs(x) < 12 else Paint("body", -0.12)
        return Paint("body", 0.04)
    return Paint("body", -0.04)


def sb_hood(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top:
        return Paint("stripe", 0.08)
    if c.bottom:
        return Paint("stripe", -0.4)
    if c.front and abs(x) < 8.5 and iy in (24, 25, 28):
        if math.floor(abs(x)) == 0:
            return Paint("stripe", -0.2)
        return glass(iy, x, iy, top=29, bottom=24)
    if c.side and iy == 25 and 20 <= z < 25:
        return glass(iy, z, iy, top=29, bottom=24)
    return Paint("stripe", -0.08 if iy < 24 else 0.0)


def sb_sail(c):
    x, y, z = c.p
    if not c.side:
        return SAIL_SHADE
    return SAIL


def sb_band(edge, battens):
    """A sail's slanted edge: white, a tape along the outer edge, battens across it."""
    def paint(c):
        if not c.side:
            return SAIL_SHADE
        lz = c.loc[2] - c.box.frm[2]
        ly = c.loc[1] - c.box.extra.get("y0", c.box.frm[1])
        out = (c.box.to[2] - c.loc[2]) if edge == "aft" else lz
        if out < 1.0:
            return SAIL_SEAM
        if battens and math.floor(ly) % 30 == 15 and out < 6.5:
            return SAIL_SHADE
        return SAIL
    return paint


def sb_mast(c):
    x, y, z = c.p
    if c.top or c.bottom:
        return MAST_DARK
    if c.side and math.floor(z) == math.floor(MAST_Z[1]) - 1:
        return MAST_DARK
    return MAST


def build_sailboat():
    boxes = []
    add = boxes.append
    hw = SB_HALF
    v, st = SB_BOW, SB_STERN
    # Hull: a long raked bow, a parallel middle under the cabin and a stern drawn in to a
    # transom, the cockpit sunk into it.
    boxes += layered_end(v, "bow", [(-2, 4, 13.0), (4, 9, 6.5), (9, SB_BOW_SHEER, 0.0)], "hull", "hull",
                         deck=SB_DECK, sheer=SB_BOW_SHEER)
    add(Box("midship", (-hw, -2, v.join), (hw, SB_DECK, -v.join), "hull", sheer=SB_SHEER))
    # The toe rail a little under the quarters' rail, so the sheer lifts aft.
    boxes += pair(Box("toe_rail_right", (hw - 1, SB_DECK, v.join), (hw, SB_SHEER - LIFT, -v.join), "hull", sheer=SB_SHEER))
    boxes += pair(zflip(st.slab("quarter_right", -2 + LIFT, SB_SHEER, "hull", sheer=SB_SHEER, deckwide=True)))
    add(zflip(st.post("transom", -2, SB_DECK, "hull", sheer=SB_SHEER)))
    boxes += [zflip(b) for b in st.fill("cockpit", -2, SB_SOLE, "hull", st.tip + 6.0, sheer=SB_SHEER)]
    # Under water: the canoe body, a fin keel and the rudder.
    add(Box("canoe", (-20, -5, -40), (20, -2, 70), "hull"))
    add(Box("keel", (-2, -11, -12), (2, -5, 12), "hull"))
    add(Box("rudder", (-1, -9, 66), (1, -5, 72), "hull"))
    # The cabin trunk with its raked front, the sprayhood over the companionway.
    tw = 17.0
    add(Box("trunk", (-tw, SB_DECK, SB_CABIN[0]), (tw, SB_TRUNK_TOP, SB_CABIN[1]), "trunk"))
    rake = math.degrees(math.atan2(6.0, SB_TRUNK_TOP - SB_DECK))
    add(Box("trunk_front", (-tw + 0.25, SB_DECK, SB_CABIN[0] - 6), (tw - 0.25, SB_DECK + (SB_TRUNK_TOP - SB_DECK) / math.cos(math.radians(rake)), SB_CABIN[0]),
            "front", origin=(0, SB_DECK, SB_CABIN[0] - 6), rot=(rake, 0, 0)))
    add(Box("sprayhood", (-11, SB_TRUNK_TOP, 17), (11, 28, 26), "hood"))
    add(Box("sprayhood_top", (-10, 28, 19.5), (10, 30, 26), "hood"))
    # Benches along the cockpit and a tiller.
    boxes += pair(Box("bench_right", (13, SB_SOLE, 30), (21, 11.5, 66), "bench"))
    add(Box("tiller", (-0.6, 11, 50), (0.6, 12.2, 74), "wood"))
    # Rig: mast, spreaders, shrouds, boom, forestay and backstay.
    mid = (SB_TRUNK_TOP - 1 + 200) / 2
    add(Box("mast_lower", (-1.25, SB_TRUNK_TOP - 1, MAST_Z[0]), (1.25, mid, MAST_Z[1]), "mast"))
    add(Box("mast_upper", (-1.25, mid, MAST_Z[0]), (1.25, 200, MAST_Z[1]), "mast"))
    add(Box("masthead_light", (-1.25, 200, MAST_Z[0]), (1.25, 202, MAST_Z[1]), "white_light", lamp="glow"))
    add(Box("spreaders", (-15, 120, -22.25), (15, 121, -21.25), "mast"))
    lo = math.degrees(math.atan2(10.0, 106.0))
    up = math.degrees(math.atan2(13.75, 76.0))
    boxes += pair(Box("shroud_right", (24.7, 14, -21.95), (25.3, 14 + math.hypot(10, 106), -21.55), "wire",
                      origin=(25, 14, -21.75), rot=(0, 0, lo)))
    boxes += pair(Box("shroud_upper_right", (14.7, 120.5, -21.95), (15.3, 120.5 + math.hypot(13.75, 76), -21.55), "wire",
                      origin=(15, 120.5, -21.75), rot=(0, 0, up)))
    add(Box("boom", (-1.25, 40, MAST_Z[1]), (1.25, 43, 60), "mast"))
    back = math.degrees(math.atan2(99.0, 187.0))
    stay = math.hypot(99, 187) / 2
    for k in range(2):
        add(Box(f"backstay{k}", (-0.2, 13 + k * stay, 77.7), (0.2, 13 + (k + 1) * stay, 78.3), "wire",
                origin=(0, 13, 78), rot=(-back, 0, 0)))
    # Mainsail: steps from the mast, the leech a turned band whose width hides their corners.
    clew, head_y, head_z, foot = 57.0, 192.0, -15.0, 43.0
    tan = (clew - head_z) / (head_y - foot)
    alpha = math.degrees(math.atan(tan))
    leech = math.hypot(clew - head_z, head_y - foot)
    for k in range(2):
        add(Box(f"main_leech{k}", (-0.75, foot + k * leech / 2, clew - 6), (0.75, foot + (k + 1) * leech / 2, clew), "main_band",
                origin=(0, foot, clew), rot=(-alpha, 0, 0), y0=foot))
    n = math.ceil((head_y - foot) / 11.5)
    h = (head_y - foot) / n
    for k in range(n):
        y0, y1 = foot + k * h, foot + (k + 1) * h
        zb = clew - (y1 - foot) * tan - 0.15
        add(Box(f"main_{k}", (-0.5, y0, MAST_Z[1] - 0.5), (0.5, y1, zb), "sail"))
    # Jib: the luff a band along the forestay, the leech upright ahead of the mast.
    stem_head = (-78.0, 15.0)
    hound = (MAST_Z[0], 182.0)
    ftan = (hound[0] - stem_head[0]) / (hound[1] - stem_head[1])
    beta = math.degrees(math.atan(ftan))
    zf = lambda y: stem_head[0] + (y - stem_head[1]) * ftan
    jib_foot, jib_leech, w = 19.0, -27.5, 5.0
    jib_top = stem_head[1] + (jib_leech - w * math.cos(math.radians(beta)) - stem_head[0]) / ftan
    luff = (jib_top - jib_foot) / math.cos(math.radians(beta))
    for k in range(2):
        add(Box(f"jib_luff{k}", (-0.75, jib_foot + k * luff / 2, zf(jib_foot)), (0.75, jib_foot + (k + 1) * luff / 2, zf(jib_foot) + w),
                "jib_band", origin=(0, jib_foot, zf(jib_foot)), rot=(beta, 0, 0)))
    n = math.ceil((jib_top - jib_foot) / 14.0)
    h = (jib_top - jib_foot) / n
    for k in range(n):
        y0, y1 = jib_foot + k * h, jib_foot + (k + 1) * h
        add(Box(f"jib_{k}", (-0.5, y0, zf(y1) + 0.15), (0.5, y1, jib_leech), "sail"))
    # The head: a few short steps up the forestay to a point.
    head = stem_head[1] + (jib_leech - stem_head[0]) / ftan
    steps = 5
    for k in range(steps):
        y0 = jib_top + k * (head - jib_top) / steps
        y1 = jib_top + (k + 1) * (head - jib_top) / steps
        front = zf((y0 + y1) / 2)
        if front < jib_leech - 0.5:
            add(Box(f"jib_head_{k}", (-0.5, y0, front), (0.5, y1, jib_leech), "sail"))
    stay = (hound[1] - jib_top) / math.cos(math.radians(beta))
    add(Box("forestay", (-0.2, jib_top, zf(jib_top) - 0.3), (0.2, jib_top + stay, zf(jib_top) + 0.3), "wire",
            origin=(0, jib_top, zf(jib_top)), rot=(beta, 0, 0)))
    # Lights: bicolour at the stem, white at the stern.
    add(Box("bow_light", (-1.2, SB_BOW_SHEER + LIFT, -79.5), (1.2, SB_BOW_SHEER + LIFT + 1.5, -76.5), "nav", lamp="glow"))
    add(Box("stern_light", (-1, SB_DECK, 78), (1, SB_DECK + 1.5, 80), "white_light", lamp="glow"))

    painters = {
        "hull": sb_hull,
        "trunk": sb_trunk,
        "front": sb_front,
        "hood": sb_hood,
        "bench": lambda c: planks(c.x, c.z) if c.top else Paint("body", -0.1),
        "wood": lambda c: WOOD if c.top else WOOD_DARK,
        "mast": sb_mast,
        "wire": lambda c: MAST_DARK,
        "sail": sb_sail,
        "main_band": sb_band("aft", True),
        "jib_band": sb_band("fore", False),
        "nav": nav_side,
        "white_light": lambda c: NAV_WHITE,
    }
    # Seat from ship_bodies.rs: 2.5 m aft of the middle, eyes 2.2 m over the water.
    return Model("ship_sailboat", boxes, painters,
                 {"body": "minecraft:white_concrete", "lower": "minecraft:blue_concrete",
                  "stripe": "minecraft:white_concrete", "door": "minecraft:light_gray_terracotta",
                  "roof": "minecraft:white_concrete"},
                 eye=(0.0, 2.2 * 16, 2.5 * 16))



# ---------------------------------------------------------------- shared superstructure painters

HAZARD = hexc("#e8b520")
DOOR_DARK = hexc("#2b2d33")


def pane_glass(u, v, w, h):
    """One pane of tinted glass, `u` across it and `v` up it in texels, `w` by `h`: lighter
    towards the top, and a glint of two short strokes in its upper corner, so big glazed
    areas stay calm."""
    c = mix(GLASS_DARK, GLASS_MID, v / max(1.0, h))
    iu, it = math.floor(u), math.floor(h - v)
    if w >= 6 and h >= 6:
        if 1 <= iu <= 3 and iu + it == 5:
            return GLASS_HI
        if 3 <= iu <= 4 and iu + it == 8:
            return GLASS_LIGHT
    return c


def windows(c, a, y, bay, pane, y0, y1, frame_slot="body", k=0.0):
    """A band of windows from row y0 to y1 (inclusive), `pane` texels wide every `bay`, `a`
    the spot along the wall: glass, or the wall between them."""
    iy = rows(y)
    if y0 <= iy <= y1:
        ia = math.floor(a) % bay
        if ia < pane:
            return pane_glass(a % bay, y - y0, pane, y1 + 1 - y0)
        return Paint(frame_slot, k - 0.06)
    return None


def car_painter(colour):
    dark = mix(colour, (0, 0, 0), 0.35)
    def paint(c):
        x, y, z = c.p
        b = c.box
        part = b.extra.get("part")
        iy = rows(y)
        if part == "wheel":
            if c.side:
                wz = (b.frm[2] + b.to[2]) / 2
                wy = (b.frm[1] + b.to[1]) / 2
                return wheel_face(z - wz, y - wy, radius=4.5)
            return TRIM
        if part == "cabin":
            if c.top:
                return mix(colour, (255, 255, 255), 0.1)
            if c.side:
                if iy >= b.to[1] - 1:
                    return colour
                return glass(y, z, y, top=b.to[1], bottom=b.frm[1])
            return glass(y, x, y, top=b.to[1], bottom=b.frm[1])
        if c.top:
            return mix(colour, (255, 255, 255), 0.08)
        if c.bottom:
            return UNDER
        if c.front or c.back:
            ax = abs(x - (b.frm[0] + b.to[0]) / 2)
            if iy == math.floor(b.frm[1]) + 5 and ax > 8:
                return hexc("#f7fbff") if c.front else hexc("#c4252e")
            if iy <= math.floor(b.frm[1]) + 1:
                return TRIM
            return colour
        if iy == math.floor(b.frm[1]) + 1:
            return TRIM
        if iy == math.floor(b.frm[1]) + 6:
            return dark
        return colour
    return paint


def parked_car(name, x, z, mat, deck):
    """A small car standing on a deck, 4.4 m long, nose towards -z."""
    out = [
        Box(f"{name}_body", (x - 14, deck + 3, z - 35), (x + 14, deck + 12, z + 35), mat, part="body"),
        Box(f"{name}_cabin", (x - 12.5, deck + 12, z - 14), (x + 12.5, deck + 21, z + 17), mat, part="cabin"),
    ]
    for k, wz in enumerate((z - 22, z + 22)):
        out.append(Box(f"{name}_axle{k}", (x - 14.5, deck, wz - 5), (x + 14.5, deck + 9, wz + 5), mat, part="wheel"))
    return out


# ---------------------------------------------------------------- river ferry

RF_HALF = 9.0 * 8          # 72
RF_DECK = 10
RF_END = 172.0             # where the pontoon stops and the ramps start
RF_SEG = 86.0
RF_HOUSE = (38.0, 70.0)    # the wheelhouse across, on the right side


def rf_hull(c):
    x, y, z = c.p
    iy = rows(y)
    if c.bottom:
        return ANTIFOUL_DARK
    if c.top:
        # The car deck: two lanes marked out, a walkway along each side.
        ax, ix = abs(x), math.floor(x)
        if ax > RF_HALF - 6:
            return Paint("door", 0.12 if math.floor(ax) == RF_HALF - 6 else 0.04)
        if ix in (-14, 32):
            return hexc("#d8d9d4")
        if ix in (-RF_HALF + 7, RF_HALF - 8):
            return HAZARD
        return Paint("door", -0.02 if ix % 2 else 0.0)
    if iy < 0:
        return ANTIFOUL if iy >= -3 else ANTIFOUL_DARK
    if iy < 2:
        return Paint("stripe", -0.06)
    if 5 <= iy <= 7:
        return Paint("stripe", 0.06 if iy == 7 else -0.02)
    if iy >= 8:
        return Paint("lower", 0.1)
    return Paint("lower", -0.08 if iy == 2 else 0.0)


def rf_fender(c):
    return FENDER_HI if c.top or rows(c.y) == 9 else FENDER


def rf_ramp(c):
    lz = c.loc[2] - c.box.frm[2]
    if c.top:
        if lz > c.box.to[2] - c.box.frm[2] - 2:
            return HAZARD
        return Paint("door", -0.18) if math.floor(lz) % 3 == 0 else Paint("door", 0.04)
    if c.front or c.back:
        return HAZARD if math.floor(abs(c.x)) % 8 < 4 else TRIM
    if c.side:
        return Paint("door", -0.25)
    return Paint("door", -0.35)


def rf_portal(c):
    x, y, z = c.p
    if c.top:
        return Paint("stripe", 0.1)
    if c.bottom:
        return Paint("stripe", -0.35)
    if rows(y) >= 42 and not c.side:
        return Paint("stripe", 0.0)
    return Paint("stripe", -0.08 if c.side else 0.0)


def rf_tower(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top:
        return Paint("roof", 0.06)
    if c.bottom:
        return UNDER
    # The door on the inboard side and a porthole on the others.
    if c.side and c.n[0] < 0:
        if abs(z) < 5 and iy < RF_DECK + 15:
            return DOOR_DARK if abs(z) < 4 else Paint("body", -0.25)
    if 22 <= iy <= 25:
        a = c.z if c.side else c.x
        if abs((a % 16) - 8) < 2.1:
            return glass(y, a, y, top=26, bottom=22) if abs((a % 16) - 8) < 1.6 else Paint("body", -0.18)
    return Paint("body", -0.06 if iy < RF_DECK + 2 else 0.0)


def rf_house(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top:
        return Paint("body", 0.1)
    if c.bottom:
        return Paint("body", -0.3)
    if iy == 31 or iy == 32:
        return Paint("stripe", 0.0 if iy == 32 else -0.1)
    a = c.z if c.side else c.x
    w = windows(c, a + 64, y, 8, 7, 36, 44)
    if w is not None:
        return w
    if iy == 45:
        return Paint("body", 0.08)
    return Paint("body", 0.0)


def rf_roof(c):
    if c.top:
        return Paint("roof", 0.08 if rows(c.y) else 0.0)
    if c.bottom:
        return Paint("roof", -0.35)
    return Paint("roof", -0.05)


RING_ORANGE = hexc("#f2672a")
RING_WHITE = hexc("#f1efe9")


def ring_painter(c):
    """A life ring on a wall: orange and white quarters round a dark middle."""
    x, y, z = c.p
    b = c.box
    cy = (b.frm[1] + b.to[1]) / 2
    flat = 0 if (b.to[0] - b.frm[0]) < (b.to[2] - b.frm[2]) else 2
    if (flat == 0 and not c.side) or (flat == 2 and not (c.front or c.back)):
        return RING_ORANGE
    ca = (b.frm[2] + b.to[2]) / 2 if flat == 0 else (b.frm[0] + b.to[0]) / 2
    dy, dz = y - cy, (z if flat == 0 else x) - ca
    if abs(dy) < 1.2 and abs(dz) < 1.2:
        return mix(RAIL_DARK, (0, 0, 0), 0.4)
    return RING_WHITE if (dy > 0) == (dz > 0) else RING_ORANGE


def build_river_ferry():
    boxes = []
    add = boxes.append
    hw = RF_HALF
    # The pontoon in four alike segments, its ends shallower over the slope up to the ramps.
    ph = hw - 0.6
    boxes += run(Box("pontoon", (-ph, -12.8, -RF_SEG), (ph, RF_DECK, 0), "hull"), 2, RF_SEG)
    end = Box("pontoon_end", (-ph, -6, -RF_END), (ph, RF_DECK, -RF_SEG), "hull")
    step = Box("pontoon_step", (-ph, -12.8, -RF_END + 44), (ph, -6, -RF_SEG), "hull")
    boxes += [end, step, zflip(end, "pontoon_end_aft"), zflip(step, "pontoon_step_aft")]
    boxes += pair(Box("fender_right", (ph, 7, -RF_SEG), (hw, 10, 0), "fender"))
    boxes += pair(Box("fender_aft_right", (ph, 7, 0), (hw, 10, RF_SEG), "fender"))
    boxes += pair(Box("fender_end_right", (ph, 7, -RF_END + 8), (hw, 10, -RF_SEG), "fender"))
    boxes += pair(zflip(Box("fender_end_aft_right", (ph, 7, -RF_END + 8), (hw, 10, -RF_SEG), "fender")))
    # Bollards at the four corners, life rings on the railings.
    for z in (-150.0, 150.0):
        boxes += pair(Box(f"bollard_{'aft' if z > 0 else 'fore'}_right", (ph - 9, RF_DECK, z - 2), (ph - 5, RF_DECK + 4, z + 2), "bollard"))
    for z in (-102.0, 102.0):
        boxes += pair(Box(f"life_ring_{'aft' if z > 0 else 'fore'}_right", (ph - 2.75, 15, z - 3.5), (ph - 2.25, 22, z + 3.5), "ring"))
    for sign, tag in ((1, "aft"), (-1, "fore")):
        ramp = Box(f"ramp_{tag}", (-58, RF_DECK - 2, RF_END), (58, RF_DECK, RF_END + 19.8), "ramp",
                   origin=(0, RF_DECK, RF_END), rot=(-15, 0, 0))
        post = Box(f"portal_post_{tag}_right", (55, RF_DECK, RF_END - 6), (59, 44, RF_END - 2), "portal")
        beam = Box(f"portal_beam_{tag}", (-59, 44, RF_END - 6.5), (59, 47, RF_END - 1.5), "portal")
        hoist = Box(f"hoist_{tag}_right", (49.5, 13, RF_END + 17.5), (50.5, 13 + 34.5, RF_END + 18.5), "wire",
                    origin=(50, 13, RF_END + 18), rot=(-36, 0, 0))
        parts = [ramp, beam] + pair(post) + pair(hoist)
        boxes += parts if sign > 0 else [zflip(b) for b in parts]
    # Railings along both sides: posts and two rails.
    for k in range(9):
        z = -136 + k * 34
        boxes += pair(Box(f"stanchion_{k}_right", (hw - 2, RF_DECK, z - 0.5), (hw - 1, 26, z + 0.5), "rail"))
    for name, y0, y1 in (("rail_top", 25, 26.5), ("rail_mid", 17.5, 18.5)):
        for k, (z0, z1) in enumerate(((-137, 0), (0, 137))):
            boxes += pair(Box(f"{name}{k}_right", (hw - 2.25, y0, z0), (hw - 0.75, y1, z1), "rail"))
    # The wheelhouse on its tower at the right side, glazed all round.
    x0, x1 = RF_HOUSE
    add(Box("tower", (x0 + 6, RF_DECK, -16), (x1 - 2, 31, 16), "tower"))
    add(Box("wheelhouse", (x0, 31, -24), (x1, 48, 24), "house"))
    add(Box("house_roof", (x0 - 2, 48, -26), (hw, 50.5, 26), "roof"))
    add(Box("light_mast", (55, 50.5, -1), (57, 53.5, 1), "rail"))
    add(Box("masthead_light", (54.5, 53.5, -1.5), (57.5, 55, 1.5), "white_light", lamp="glow"))
    add(Box("sidelight_left", (x0 - 2, 50.5, -2), (x0 - 0.5, 52, 2), "nav", lamp="glow"))
    add(Box("sidelight_right", (hw - 1.5, 50.5, -2), (hw, 52, 2), "nav", lamp="glow"))
    # Two cars on the deck, where ship_bodies.rs parks them.
    boxes += parked_car("car_blue", -hw + 35.2, 72.0, "car_blue", RF_DECK)
    boxes += parked_car("car_white", -hw + 35.2, -32.0, "car_white", RF_DECK)

    painters = {
        "hull": rf_hull,
        "fender": rf_fender,
        "ramp": rf_ramp,
        "portal": rf_portal,
        "wire": lambda c: STEEL,
        "bollard": lambda c: FENDER_HI if c.top else FENDER,
        "ring": ring_painter,
        "rail": lambda c: RAIL if c.top else RAIL_DARK if c.bottom else mix(RAIL, RAIL_DARK, 0.3),
        "tower": rf_tower,
        "house": rf_house,
        "roof": rf_roof,
        "white_light": lambda c: NAV_WHITE,
        "nav": nav_side,
        "car_blue": car_painter(hexc("#2f5fae")),
        "car_white": car_painter(hexc("#e3e6ea")),
    }
    # Middle seat from ship_bodies.rs: on deck amidships, eyes 2.3 m over the water.
    return Model("ship_river_ferry", boxes, painters,
                 {"body": "minecraft:white_concrete", "lower": "minecraft:red_concrete",
                  "stripe": "minecraft:red_concrete", "door": "minecraft:gray_concrete",
                  "roof": "minecraft:red_concrete"},
                 eye=(0.0, 2.3 * 16, 0.0))



# ---------------------------------------------------------------- water bus

WB_HALF = 7.0 * 8          # 56
WB_END = VEnd(tip=-222.5, join=-134.4, half=WB_HALF, stem=2.0, thick=4.0)
WB_DECK, WB_SHEER, WB_BULWARK = 16, 20, 30
WB_CABIN = (136.0, 48.8, 51.2)   # half length, half width, roof
WB_BAY = 34.0
WB_STRIPE = (11, 15)


def wb_hull(c):
    x, y, z = c.p
    iy = rows(y)
    if c.bottom:
        return ANTIFOUL_DARK
    if c.top:
        if y > WB_SHEER + 1:
            return Paint("body", 0.1)
        if y > WB_DECK + 0.5:
            return RUB
        return planks(x, z, 4, None, slot="door")
    if inward(c):
        return Paint("body", -0.1 if rows(y) < WB_SHEER else -0.04)
    if iy >= WB_SHEER:
        if iy == WB_BULWARK - 1:
            return RUB
        if iy == WB_SHEER:
            return Paint("stripe", 0.0)
        return Paint("body", -0.02 if iy > WB_SHEER + 2 else -0.08)
    if iy == WB_SHEER - 1:
        return RUB
    return hull_bands(y, 99, WB_STRIPE, rub=False)


def wb_cabin(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top:
        return Paint("body", 0.1)
    if c.bottom:
        return UNDER
    ht = WB_CABIN[0]
    if c.side:
        # Doors at both ends, a band of big windows between.
        if abs(z) > ht - WB_BAY:
            dz = (ht - abs(z))
            if 6 <= dz < 22 and WB_DECK + 1 <= iy <= 45:
                if math.floor(dz) in (6, 13, 14, 21):
                    return TRIM
                return glass(y, dz, iy, top=46, bottom=WB_DECK + 1)
        w = windows(c, z + 1024, y, int(WB_BAY), int(WB_BAY) - 3, 28, 45)
        if w is not None:
            return w
        if iy == 27:
            return Paint("stripe", 0.0)
        if iy == 46 or iy == 26:
            return Paint("body", 0.06)
        return Paint("body", -0.04 if iy < 26 else 0.0)
    # The ends: three windows and a door in the middle.
    ax = abs(x)
    if ax < 7 and WB_DECK + 1 <= iy <= 45:
        if math.floor(ax) == 0:
            return TRIM
        return glass(y, ax, iy, top=46, bottom=WB_DECK + 1)
    if 28 <= iy <= 45 and 10 <= ax < 46:
        if math.floor(ax) % 18 in (10, 11):
            return Paint("body", -0.06)
        return glass(y, ax, iy, top=46, bottom=28)
    if iy == 27:
        return Paint("stripe", 0.0)
    return Paint("body", -0.04 if iy < 26 else 0.0)


def wb_roof(c):
    x, y, z = c.p
    if c.top:
        ax = abs(x)
        if ax > WB_CABIN[1] - 2.5 or abs(z) > WB_CABIN[0] - 1:
            return Paint("roof", 0.12)
        if y > 60:
            return Paint("roof", 0.06)
        # The sun deck inside the roof's coloured rim.
        return planks(x, z, 4, None, k=0.04)
    if c.bottom:
        return Paint("roof", -0.4)
    return Paint("roof", -0.06 if rows(y) == math.floor(c.box.frm[1]) else 0.04)


def wb_house(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top:
        return Paint("body", 0.1)
    if c.bottom:
        return Paint("body", -0.3)
    a = c.z if c.side else c.x
    w = windows(c, a + 64, y, 7, 6, 64, 74)
    if w is not None:
        return w
    if iy == 63:
        return Paint("stripe", 0.0)
    return Paint("body", 0.0 if iy > 58 else -0.04)


def build_water_bus():
    boxes = []
    add = boxes.append
    hw = WB_HALF
    ph = hw - 0.6
    v = WB_END
    # Hull: pointed and alike at both ends; amidships four alike segments.
    seg = -2 * v.join / 4
    boxes += run(Box("hull", (-ph, -12, v.join), (ph, WB_DECK, v.join + seg), "hull"), 4, seg)
    boxes += run(Box("bulwark_right", (ph - 3, WB_DECK, v.join), (ph, WB_SHEER, v.join + seg), "hull"), 4, seg)
    boxes += run(Box("bulwark_left", (-ph, WB_DECK, v.join), (-ph + 3, WB_SHEER, v.join + seg), "hull"), 4, seg)
    boxes += pair(Box("fender_right", (ph, 13, v.join), (hw, 16, 0), "fender"))
    boxes += pair(Box("fender_aft_right", (ph, 13, 0), (hw, 16, -v.join), "fender"))
    add(Box("keel", (-40, -19.2, v.join + 20), (40, -12, -v.join - 20), "hull"))
    end = VEnd(tip=v.tip, join=v.join, half=ph, stem=v.stem, thick=v.thick)
    fore = layered_end(end, "end_fore", [(-12, 6, 8.0), (6, WB_BULWARK, 0.0)], "hull", "hull", deck=WB_DECK)
    boxes += fore + [zflip(b, b.name.replace("fore", "aft")) for b in fore]
    for sign, tag in ((-1, "fore"), (1, "aft")):
        fender = Box(f"stem_fender_{tag}", (-2.5, 2, -224), (2.5, WB_BULWARK - 2, -221.5), "fender")
        add(fender if sign < 0 else zflip(fender))
    # The cabin, glazed along its length, in four alike segments, and its roof.
    ht, cw, top = WB_CABIN
    boxes += run(Box("cabin", (-cw, WB_DECK, -ht), (cw, top, -ht + ht / 2), "cabin"), 4, ht / 2)
    # The roof lies exactly over the cabin, so a rider inside sees no ceiling; its eaves
    # are strips of their own.
    boxes += run(Box("roof", (-cw, top, -ht), (cw, top + 3.2, -ht + ht / 2), "roof"), 4, ht / 2)
    boxes += pair(Box("eave_right", (cw, top, -ht - 6), (cw + 1.6, top + 3.2, 0), "roof"))
    boxes += pair(Box("eave_aft_right", (cw, top, 0), (cw + 1.6, top + 3.2, ht + 6), "roof"))
    eave = Box("eave_fore", (-cw, top, -ht - 6), (cw, top + 3.2, -ht), "roof")
    boxes += [eave, zflip(eave, "eave_aft")]
    # Sun deck railings round the roof.
    for k in range(5):
        z = -128 + k * 64
        boxes += pair(Box(f"roof_post_{k}_right", (cw - 0.5, top + 3.2, z - 0.5), (cw + 0.5, top + 17, z + 0.5), "rail"))
    for k, (z0, z1) in enumerate(((-129, 0), (0, 129))):
        boxes += pair(Box(f"roof_rail{k}_right", (cw - 0.75, top + 16, z0), (cw + 0.75, top + 17.5, z1), "rail"))
    # Wheelhouse in the middle of the roof, glazed all round.
    add(Box("wheelhouse", (-20.8, top + 3.2, -20.8), (20.8, 78.4, 20.8), "house"))
    add(Box("house_roof", (-24, 78.4, -24), (24, 81.6, 24), "roof"))
    add(Box("light_mast", (-1, 81.6, -1), (1, 84, 1), "rail"))
    add(Box("masthead_light", (-1.5, 84, -1.5), (1.5, 85.25, 1.5), "white_light", lamp="glow"))
    boxes += pair(Box("sidelight_right", (21.6, 70, -2), (22.4, 73, 2), "nav", lamp="glow"))
    # Bollards and life rings on the open decks.
    for sign, tag in ((-1, "fore"), (1, "aft")):
        boxes += pair(Box(f"bollard_{tag}_right", (36, WB_DECK, sign * 160 - 2), (40, WB_DECK + 4, sign * 160 + 2), "bollard"))
        ring = Box(f"life_ring_{tag}_right", (36.5, 24, -WB_CABIN[0] - 0.5), (43.5, 31, -WB_CABIN[0]), "ring")
        boxes += pair(ring if sign < 0 else zflip(ring))
        bench = Box(f"bench_{tag}", (-7, top + 3.2, -112), (7, top + 6.5, -44), "bench")
        backrest = Box(f"bench_back_{tag}", (-1, top + 6.5, -112), (1, top + 10, -44), "bench")
        raft = Box(f"raft_{tag}_right", (30, top + 3.2, -34), (40, top + 8, -26), "raft")
        for b in [bench, backrest] + pair(raft):
            add(b if sign < 0 else zflip(b))

    painters = {
        "hull": wb_hull,
        "fender": rf_fender,
        "cabin": wb_cabin,
        "roof": wb_roof,
        "house": wb_house,
        "rail": lambda c: RAIL if c.top else mix(RAIL, RAIL_DARK, 0.3),
        "white_light": lambda c: NAV_WHITE,
        "nav": nav_side,
        "bollard": lambda c: FENDER_HI if c.top else FENDER,
        "ring": ring_painter,
        "bench": lambda c: planks(c.z, c.x, 2, None, k=0.1) if c.top else Paint("door", -0.15),
        "raft": lambda c: RING_WHITE if not c.side or rows(c.y) % 3 else hexc("#c9c6bd"),
    }
    # Middle seat from ship_bodies.rs: in the cabin amidships, eyes 2.7 m over the water.
    return Model("ship_water_bus", boxes, painters,
                 {"body": "minecraft:white_concrete", "lower": "minecraft:blue_concrete",
                  "stripe": "minecraft:blue_concrete", "door": "minecraft:gray_concrete",
                  "roof": "minecraft:blue_concrete"},
                 eye=(0.0, 2.7 * 16, 0.0))



# ---------------------------------------------------------------- tour boat

TB_HALF = 8.0 * 8          # 64
TB_BOW = VEnd(tip=-320.0, join=-240.0, half=TB_HALF - 0.6, stem=2.0, thick=4.0)
TB_STERN = VEnd(tip=-320.0, join=-216.0, half=TB_HALF - 0.6, stem=54.0, thick=6.0)
TB_DECK, TB_BULWARK = 17, 23
TB_CABIN = (-232.0, 216.0)
TB_BAY = 32.0
TB_WALL, TB_EAVE, TB_ROOF = 58.0, 44.0, 54.4     # cabin half width, where the glass roof bends, top
TB_FLAT = TB_WALL - (TB_ROOF - TB_EAVE)           # half width of the flat roof
TB_STRIPE = (11, 14)


def tb_hull(c):
    x, y, z = c.p
    iy = rows(y)
    if c.bottom:
        return ANTIFOUL_DARK
    if c.top:
        if y > TB_DECK + 0.5:
            return RUB
        return planks(x, z, 4, None)
    if inward(c):
        return Paint("body", -0.1)
    if iy >= TB_DECK:
        return RUB if iy == TB_BULWARK - 1 else Paint("lower", 0.06)
    if iy == TB_DECK - 1:
        return RUB
    return hull_bands(y, 99, TB_STRIPE, rub=False)


def tb_cabin(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top or c.bottom:
        return UNDER
    if c.side:
        a = (z - TB_CABIN[0]) % TB_BAY
        if iy == TB_EAVE - 1:
            return Paint("roof", 0.0)
        if 25 <= iy < TB_EAVE - 1:
            if math.floor(a) in (0, 1):
                return Paint("roof", -0.1)
            return pane_glass(a - 2, y - 25, TB_BAY - 2, TB_EAVE - 1 - 25)
        if iy == 24:
            return Paint("stripe", 0.0)
        return Paint("body", 0.04 if iy == 23 else -0.04)
    # The ends: big panes either side of a door.
    ax = abs(x)
    if 25 <= iy < TB_EAVE - 1 or (ax < 7 and TB_DECK + 1 <= iy < TB_EAVE - 1):
        if math.floor(ax) in (0, 7, 8, 30, 31) or ax > TB_WALL - 2:
            return Paint("roof", -0.1)
        lo = 1 if ax < 7 else 9 if ax < 30 else 32
        return pane_glass(ax - lo, y - (TB_DECK + 1 if ax < 7 else 25), 6 if ax < 7 else 21, TB_EAVE - 1 - (TB_DECK + 1 if ax < 7 else 25))
    if iy == TB_EAVE - 1:
        return Paint("roof", 0.0)
    if iy == 24:
        return Paint("stripe", 0.0)
    return Paint("body", -0.04)


def tb_chamfer(c):
    """The bent edge of the glass roof, a slab turned 45 degrees about z."""
    if c.face == "up":
        a = (c.p[2] - TB_CABIN[0]) % TB_BAY
        across = c.loc[0] - c.box.frm[0]
        w = c.box.to[0] - c.box.frm[0]
        if math.floor(a) in (0, 1) or across > w - 1.2:
            return Paint("roof", -0.05)
        return pane_glass(a - 2, across, TB_BAY - 2, w - 1.2)
    return Paint("roof", -0.2)


def tb_roof(c):
    x, y, z = c.p
    if c.top:
        ax = abs(x)
        a = (z - TB_CABIN[0]) % TB_BAY
        if ax < 10:
            return Paint("roof", 0.06 if math.floor(ax) != 9 else -0.08)
        if math.floor(a) in (0, 1) or ax > TB_FLAT - 1.2:
            return Paint("roof", -0.02)
        return pane_glass(a - 2, ax - 10, TB_BAY - 2, TB_FLAT - 11.2)
    if c.bottom:
        return UNDER
    return Paint("roof", -0.06)


def tb_house(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top:
        return Paint("body", 0.1)
    if c.bottom:
        return Paint("body", -0.3)
    a = c.z if c.side else c.x
    w = windows(c, a + 64, y, 9, 8, 65, 76)
    if w is not None:
        return w
    if iy == 64:
        return Paint("stripe", 0.0)
    return Paint("body", 0.0)


def build_tour_boat(low=False):
    boxes = []
    add = boxes.append
    hw = TB_HALF
    ph = hw - 0.6
    v, st = TB_BOW, TB_STERN
    # Hull: six alike segments amidships, a long pointed bow and a stern drawn in a little.
    seg = (-st.join - v.join) / 6
    boxes += run(Box("hull", (-ph, -22.4, v.join), (ph, TB_DECK, v.join + seg), "hull"), 6, seg)
    for k in range(3):
        z0 = v.join + k * 2 * seg
        boxes += pair(Box(f"fender{k}_right", (ph, 12, z0), (hw, 15, z0 + 2 * seg), "fender"))
    boxes += layered_end(v, "bow", [(-14, 6, 10.0), (6, TB_BULWARK, 0.0)], "hull", "hull", deck=TB_DECK)
    boxes += pair(zflip(st.slab("quarter_right", -14 + LIFT, TB_BULWARK, "hull")))
    add(zflip(st.post("transom", -14, TB_DECK, "hull")))
    boxes += [zflip(b) for b in st.fill("aft_deck", -14, TB_DECK, "hull", st.tip + 6.0)]
    # The cabin: glass sides in bays, the roof's glazed edges bent down at 45 degrees.
    z0, z1 = TB_CABIN
    n = 7
    cseg = (z1 - z0) / n
    boxes += run(Box("cabin", (-TB_WALL, TB_DECK, z0), (TB_WALL, TB_EAVE, z0 + cseg), "cabin"), n, cseg)
    boxes += run(Box("roof", (-TB_FLAT, TB_EAVE, z0), (TB_FLAT, TB_ROOF, z0 + cseg), "roof"), n, cseg)
    bend = TB_ROOF - TB_EAVE
    for k in range(n):
        # Flush with each other; the two at the ends stop a quarter short of the end walls.
        a = z0 + k * cseg + (0.25 if k == 0 else 0)
        b = z0 + (k + 1) * cseg - (0.25 if k == n - 1 else 0)
        boxes += pair(Box(f"glass_eave{k}_right", (TB_WALL - bend * math.sqrt(2), TB_EAVE - bend / math.sqrt(2), a),
                          (TB_WALL, TB_EAVE, b), "chamfer", origin=(TB_WALL, TB_EAVE, a), rot=(0, 0, -45)))
    # The open stern deck: benches along the sides, a railing on the bulwark.
    for k, zz in enumerate((236.0, 272.0)):
        boxes += pair(Box(f"aft_bench{k}_right", (38, TB_DECK, zz), (47, TB_DECK + 6, zz + 26), "bench"))
    reach = st.length - 8
    rails = [st.along("aft_rail_right", 1.5, 1.5, TB_BULWARK + 8, TB_BULWARK + 9.5, 8, st.length, "rail")]
    for k in range(3):
        d = 12 + k * (reach - 6) / 2
        rails.append(st.along(f"aft_post{k}_right", 1.75, 1.0, TB_BULWARK, TB_BULWARK + 8, d - 0.5, d + 0.5, "rail"))
    for b in rails:
        boxes += pair(zflip(b))
    # Foredeck: bollards and the bow light.
    boxes += pair(Box("bollard_right", (20, TB_DECK, -262), (24, TB_DECK + 4, -258), "bollard"))
    add(Box("bow_light", (-1.7, TB_BULWARK + LIFT, -319.5), (1.7, TB_BULWARK + LIFT + 1.5, -316.5), "nav", lamp="glow"))
    add(Box("stern_light", (-1.5, TB_DECK, 319.6), (1.5, TB_DECK + 2, 320), "white_light", lamp="glow"))
    if not low:
        # The top tier: railings round the roof and the wheelhouse raised over the front.
        for k in range(8):
            zz = z0 + 40 + k * 56
            if -200 < zz < -136:
                continue
            boxes += pair(Box(f"roof_post{k}_right", (TB_FLAT - 2, TB_ROOF, zz - 0.5), (TB_FLAT - 1, TB_ROOF + 14, zz + 0.5), "rail"))
        for k, (a, b) in enumerate(((-136.0, 40.0), (40.0, 214.0))):
            boxes += pair(Box(f"roof_rail{k}_right", (TB_FLAT - 2.25, TB_ROOF + 13, a), (TB_FLAT - 0.75, TB_ROOF + 14.5, b), "rail"))
        add(Box("wheelhouse", (-22.4, TB_ROOF, -192), (22.4, 78.4, -144), "house"))
        add(Box("house_roof", (-25, 78.4, -195), (25, 80, -141), "house_roof"))
        boxes += pair(Box("sidelight_right", (22.4, 70, -170), (23.2, 73, -166), "nav", lamp="glow"))
        add(Box("masthead_light", (-1.5, 80, -170), (1.5, 81.5, -167), "white_light", lamp="glow"))
    else:
        boxes += pair(Box("sidelight_right", (TB_WALL, 34, -228), (TB_WALL + 0.75, 37, -224), "nav", lamp="glow"))

    painters = {
        "hull": tb_hull,
        "fender": rf_fender,
        "cabin": tb_cabin,
        "roof": tb_roof,
        "chamfer": tb_chamfer,
        "house": tb_house,
        "house_roof": lambda c: Paint("roof", 0.06 if c.top else -0.1),
        "bench": lambda c: planks(c.z, c.x, 2, None, k=0.1) if c.top else Paint("door", -0.2),
        "rail": lambda c: RAIL if c.top else mix(RAIL, RAIL_DARK, 0.3),
        "bollard": lambda c: FENDER_HI if c.top else FENDER,
        "nav": nav_side,
        "white_light": lambda c: NAV_WHITE,
    }
    # Middle seat from ship_bodies.rs: in the cabin amidships, eyes 2.6 m over the water.
    return Model("ship_tour_boat_low" if low else "ship_tour_boat", boxes, painters,
                 {"body": "minecraft:white_concrete", "lower": "minecraft:blue_concrete",
                  "stripe": "minecraft:cyan_concrete", "door": "minecraft:light_gray_concrete",
                  "roof": "minecraft:blue_concrete"},
                 eye=(0.0, 2.6 * 16, 0.0))


def build_tour_boat_low():
    return build_tour_boat(low=True)



# ---------------------------------------------------------------- peniche

PE_HALF = 5.05 * 8         # 40.4
PE_BOW = VEnd(tip=-308.0, join=-290.0, half=PE_HALF - 0.6, stem=24.0, thick=6.0)
PE_STERN = VEnd(tip=-298.0, join=-286.0, half=PE_HALF - 0.6, stem=28.0, thick=6.0)
PE_DECK, PE_BOW_SHEER = 16, 22
PE_HOLD = (-240.0, 176.0)
PE_COAMING, PE_RIDGE = 26.0, 32.0
PE_HOLD_HALF = 34.8
PE_PANEL = 26.0
PE_CABIN = (196.0, 280.0)
PE_CABIN_TOP = 44.0
PE_STRIPE = (12, 15)
FLOWER_GREEN = hexc("#3f7d2e")
FLOWER_DARK = hexc("#2b5a20")
FLOWERS = (hexc("#e8455a"), hexc("#f4d23c"), hexc("#f7f2ea"), hexc("#d65ab4"))
PLANTER = hexc("#8a4b2a")
TAR = hexc("#211d1d")
TAR_DARK = hexc("#161314")


def pe_hull(c):
    x, y, z = c.p
    iy = rows(y)
    if c.bottom:
        return TAR_DARK
    if c.top:
        if y > PE_DECK + 0.5:
            return Paint("stripe", 0.1)
        return Paint("door", -0.35) if math.floor(abs(x)) % 5 == 0 else Paint("door", -0.25)
    if inward(c):
        return Paint("lower", 0.1)
    if iy < 0:
        return TAR if iy >= -3 else TAR_DARK
    if iy < 2:
        return Paint("stripe", -0.08)
    if PE_STRIPE[0] <= iy < PE_STRIPE[1]:
        return Paint("stripe", 0.06 if iy == PE_STRIPE[1] - 1 else -0.04)
    if iy >= PE_STRIPE[1]:
        return Paint("lower", 0.14 if iy == PE_STRIPE[1] else 0.04)
    # Portholes of the living quarters under the aft deck.
    if c.side and 6 <= iy <= 8 and PE_CABIN[0] <= z < PE_CABIN[1] + 8:
        a = (z - PE_CABIN[0]) % 20
        if 8 <= a < 11:
            return glass(y, a, y, top=9, bottom=6) if iy == 7 or 9 <= a < 10 else Paint("lower", 0.3)
    return Paint("lower", -0.08 if iy < 4 else 0.0)


def pe_coaming(c):
    x, y, z = c.p
    if c.top or c.bottom:
        return Paint("lower", 0.0)
    iy = rows(y)
    if iy == PE_COAMING - 1:
        return Paint("stripe", 0.0)
    return Paint("lower", 0.08 if iy == PE_DECK else 0.0)


def pe_cover(c):
    """Hatch covers on the hold: panels across, a dark seam between each."""
    if c.face == "up":
        a = (c.p[2] - PE_HOLD[0]) % PE_PANEL
        across = c.box.to[0] - c.loc[0]
        if math.floor(a) == 0:
            return Paint("door", -0.35)
        if across < 1.0:
            return Paint("door", -0.2)
        if math.floor(a) == 1:
            return Paint("door", 0.12)
        return Paint("door", 0.02 if across > 6 else -0.05)
    return Paint("door", -0.25)


def pe_cabin(c):
    x, y, z = c.p
    iy = rows(y)
    if c.top:
        return Paint("roof", 0.08)
    if c.bottom:
        return UNDER
    if c.side:
        w = windows(c, z - PE_CABIN[0] + 4, y, 16, 9, 30, 38)
        if w is not None:
            return w
        if iy == 29 or iy == 39:
            return Paint("body", -0.12)
        return Paint("body", 0.0)
    ax = abs(x)
    if c.back and ax < 6 and iy < 40:
        return DOOR_DARK if ax < 5 else Paint("body", -0.2)
    if 30 <= iy <= 38 and 12 <= ax < 24:
        return pane_glass(ax - 12, y - 30, 12, 9)
    return Paint("body", 0.0)


def pe_house(low):
    def paint(c):
        x, y, z = c.p
        iy = rows(y)
        if c.top:
            return Paint("roof", 0.08)
        if c.bottom:
            return Paint("roof", -0.3)
        top = c.box.to[1]
        a = c.z if c.side else c.x
        if iy >= top - 2:
            return Paint("roof", 0.0)
        w = windows(c, a + 64, y, 10, 8, int(top) - 12, int(top) - 4)
        if w is not None:
            return w
        return WOOD if (math.floor(a) % 10) not in (8, 9) else WOOD_DARK
    return paint


def pe_flowers(c):
    x, y, z = c.p
    if c.top:
        h = (math.floor(x * 7) * 31 + math.floor(z) * 17) % 11
        if h < 4:
            return FLOWERS[h]
        return FLOWER_GREEN if h % 2 else FLOWER_DARK
    if rows(y) >= c.box.to[1] - 1:
        return FLOWER_GREEN
    return PLANTER


def build_peniche(low=False):
    boxes = []
    add = boxes.append
    hw = PE_HALF
    ph = hw - 0.6
    v, st = PE_BOW, PE_STERN
    # Hull: nine alike segments, a bluff raked bow, a rounded stern; a rubbing strake.
    seg = 64.0
    boxes += run(Box("hull", (-ph, -28.8, v.join), (ph, PE_DECK, v.join + seg), "hull"), 9, seg)
    for k in range(3):
        z0 = v.join + k * 3 * seg
        boxes += pair(Box(f"strake{k}_right", (ph, 12, z0), (hw, 14, z0 + 3 * seg), "fender"))
    boxes += layered_end(v, "bow", [(-28.8, -8, 8.0), (-8, 6, 4.0), (6, PE_BOW_SHEER, 0.0)], "hull", "hull",
                         deck=PE_DECK, plate=True)
    aft = layered_end(st, "stern", [(-28.8, -4, 5.0), (-4, PE_DECK + 3, 0.0)], "hull", "hull", deck=PE_DECK, plate=True)
    boxes += [zflip(b) for b in aft]
    # The hold: a coaming and hatch covers peaked along the middle, in alike segments.
    z0, z1 = PE_HOLD
    hseg = 4 * PE_PANEL
    n = round((z1 - z0) / hseg)
    boxes += run(Box("coaming", (-PE_HOLD_HALF, PE_DECK, z0), (PE_HOLD_HALF, PE_COAMING, z0 + hseg), "coaming"), n, hseg)
    pitch = math.degrees(math.atan2(PE_RIDGE - PE_COAMING, PE_HOLD_HALF))
    run_len = math.hypot(PE_RIDGE - PE_COAMING, PE_HOLD_HALF)
    thick = PE_HOLD_HALF * (PE_RIDGE - PE_COAMING) / run_len + 0.1
    for k in range(n):
        # The end panels stop short of the coaming's ends, so the gables never lie in the
        # coaming's plane and its top shows round them.
        a = z0 + k * hseg + (0.5 if k == 0 else 0)
        b = z0 + (k + 1) * hseg - (0.5 if k == n - 1 else 0)
        boxes += pair(Box(f"cover{k}_right", (0, PE_RIDGE - thick, a), (run_len, PE_RIDGE, b), "cover",
                          origin=(0, PE_RIDGE, a), rot=(0, 0, -pitch)))
    # A ridge cap along the covers, standing out of the gables at both ends.
    for k in range(n):
        a = z0 + k * hseg + (0.2 if k == 0 else 0)
        b = z0 + (k + 1) * hseg - (0.2 if k == n - 1 else 0)
        add(Box(f"ridge{k}", (-2.5, PE_COAMING, a), (2.5, PE_RIDGE + 1, b), "ridge"))
    # Foredeck: windlass, bollards, the mast for the masthead light (laid down when low).
    add(Box("windlass", (-10, PE_DECK, -282), (10, PE_DECK + 7, -274), "windlass"))
    add(Box("windlass_drum", (-13, PE_DECK + 2, -280.5), (13, PE_DECK + 5.5, -275.5), "windlass"))
    boxes += pair(Box("bollard_fore_right", (26, PE_DECK, -264), (30, PE_DECK + 5, -260), "bollard"))
    boxes += pair(Box("bollard_aft_right", (30, PE_DECK, 286), (34, PE_DECK + 5, 290), "bollard"))
    if low:
        add(Box("bow_mast", (-1, PE_DECK + 7, -270), (1, PE_DECK + 9, -228), "mast"))
        add(Box("masthead_light", (-1.5, PE_DECK + 7, -228), (1.5, PE_DECK + 9.5, -225), "white_light", lamp="glow"))
    else:
        add(Box("bow_mast", (-1, PE_DECK, -268), (1, 66, -266), "mast"))
        add(Box("masthead_light", (-1.5, 66, -268.5), (1.5, 68.5, -265.5), "white_light", lamp="glow"))
    # Aft: the living quarters, the wheelhouse on them (sunk into them when low), the
    # rudder and its tiller, flowers along the roof.
    c0, c1 = PE_CABIN
    add(Box("cabin", (-34, PE_DECK, c0), (34, PE_CABIN_TOP, c1), "cabin"))
    add(Box("cabin_roof", (-35.5, PE_CABIN_TOP, c0 - 1.5), (35.5, PE_CABIN_TOP + 2.5, c1 + 1.5), "roof"))
    top = PE_CABIN_TOP + 2.5
    if low:
        add(Box("wheelhouse", (-20.8, top, 226), (20.8, 50.4, 266), "house"))
    else:
        add(Box("wheelhouse", (-20.8, top, 226), (20.8, 71.5, 266), "house"))
        add(Box("house_roof", (-22.5, 71.5, 224), (22.5, 73.6, 268), "house_roof"))
        boxes += pair(Box("sidelight_right", (20.8, 64, 228), (21.6, 67, 232), "nav", lamp="glow"))
    if low:
        boxes += pair(Box("sidelight_right", (34, 38, 198), (34.75, 41, 202), "nav", lamp="glow"))
    add(Box("rudder", (-1.25, -24, 298), (1.25, 18, 308), "rudder"))
    add(Box("rudder_stock", (-1.5, 18, 299), (1.5, 31, 302), "rudder"))
    add(Box("tiller", (-1, 28, 268), (1, 30, 301), "wood"))
    add(Box("stern_light", (-1.5, PE_DECK + 3 - 0.25, 296.5), (1.5, PE_DECK + 4.25, 297.5), "white_light", lamp="glow"))
    for k, zz in enumerate((204.0, 240.0)):
        boxes += pair(Box(f"flowers{k}_right", (29, top, zz), (34, top + 3.5, zz + 22), "flowers"))

    painters = {
        "hull": pe_hull,
        "fender": lambda c: Paint("lower", 0.2 if c.top else -0.2),
        "coaming": pe_coaming,
        "cover": pe_cover,
        "cabin": pe_cabin,
        "roof": lambda c: Paint("roof", 0.08 if c.top else -0.3 if c.bottom else -0.04),
        "house": pe_house(low),
        "house_roof": lambda c: Paint("roof", 0.08 if c.top else -0.12),
        "windlass": lambda c: FENDER_HI if c.top else FENDER,
        "ridge": lambda c: Paint("door", 0.1 if c.top else -0.3),
        "bollard": lambda c: FENDER_HI if c.top else FENDER,
        "mast": lambda c: Paint("body", 0.0 if not c.bottom else -0.3),
        "rudder": lambda c: Paint("lower", -0.1 if c.side else -0.3),
        "wood": lambda c: WOOD if c.top else WOOD_DARK,
        "flowers": pe_flowers,
        "nav": nav_side,
        "white_light": lambda c: NAV_WHITE,
    }
    # Seat from ship_bodies.rs: on the hatch covers amidships, eyes 3.6 m over the water.
    return Model("ship_peniche_low" if low else "ship_peniche", boxes, painters,
                 {"body": "minecraft:white_concrete", "lower": "minecraft:black_concrete",
                  "stripe": "minecraft:red_concrete", "door": "minecraft:gray_concrete",
                  "roof": "minecraft:gray_concrete"},
                 eye=(0.0, 3.6 * 16, 0.0))


def build_peniche_low():
    return build_peniche(low=True)


MODELS = [build_motorboat, build_sailboat, build_river_ferry, build_water_bus, build_tour_boat,
          build_tour_boat_low, build_peniche, build_peniche_low]
