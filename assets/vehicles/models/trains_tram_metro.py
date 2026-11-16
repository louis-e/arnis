"""A modern low-floor tram in three kinds of section, and a third-rail metro car.

Lengths, widths, floor and roof heights, window bands, doors and the pantograph follow
`src/element_processing/train_bodies.rs`, so the sections couple, meet the platforms and
clear tunnels. Rust's frame has +z ahead; here the front is at -z, so a cab section has
its cab at -z and a Rust z becomes -z. Every end without a cab carries bellows reaching
0.2 m past the body, half of the coupling gap, so neighbours meet.

A tram is cab, short, middle, short, cab (or cab, short, cab), as in the Combino: the
short sections ride on a bogie and have no doors, the middle section hangs between them
with its door and the pantograph, and a cab section has its bogie under the driver. The
bodies are bays (windows, a door, the cab) painted relative to their own box, so alike
bays share one spot of the texture.
"""
import math

from kit import (
    CHROME_DARK, DRL, GLASS_DARK, GLASS_HI, GLASS_LIGHT, GLASS_MID, GLOSS, HOUSING,
    HOUSING_DARK, LENS, STEEL, TAIL_BRIGHT, TRIM, UNDER, Box, Model, Paint, hexc,
    mix, pair,
)

M = 16.0  # units to a metre
RIDER_EYE = 2.12 * M

# ---------------------------------------------------------------- colours

BELLOWS = hexc("#1f2023")
BELLOWS_HI = hexc("#2d2e33")
FRAME = hexc("#2a2c31")
FRAME_HI = hexc("#3e4148")
FRAME_LO = hexc("#1b1c1f")
WHEEL = hexc("#33363b")
WHEEL_DISC = hexc("#2a2c30")
TREAD = hexc("#a0a5ac")
AXLE_CAP = hexc("#6c717a")
EQUIP = hexc("#2b2d31")
EQUIP_HI = hexc("#3c3f45")
VENT = hexc("#141517")
SIGN_BG = hexc("#151311")
SIGN_TXT = hexc("#ffad2e")
INSULATOR = hexc("#c9c2b2")
CARBON = hexc("#2a2a2c")
SHOE = hexc("#4a4136")
BUTTON = hexc("#3fd26f")
WIPER = hexc("#1a1b1e")
MIRROR_GLASS = hexc("#9fb2c4")
TAIL_RED = hexc("#b71c26")

TRAM_LIVERY = {  # train_bodies.rs TRAMS[0], "ty"
    "body": "minecraft:yellow_concrete",
    "lower": "minecraft:light_gray_concrete",
    "stripe": "minecraft:yellow_concrete",
    "door": "minecraft:yellow_concrete",
    "roof": "minecraft:light_gray_concrete",
}
METRO_LIVERY = {  # train_bodies.rs METROS[0], "my"
    "body": "minecraft:yellow_concrete",
    "lower": "minecraft:gray_concrete",
    "stripe": "minecraft:yellow_concrete",
    "door": "minecraft:yellow_concrete",
    "roof": "minecraft:light_gray_concrete",
}


def fl(v):
    return math.floor(v + 1e-6)


def rel(c):
    """The texel's spot from its box's lowest corner, before the box was turned."""
    b = c.box
    return c.loc[0] - b.frm[0], c.loc[1] - b.frm[1], c.loc[2] - b.frm[2]


def size(b):
    return [b.to[i] - b.frm[i] for i in range(3)]


def cols(length):
    return max(1, math.ceil(length - 1e-6))


def from_left(c):
    """How far along a side face from the left edge as one looks at it, so both sides of
    a symmetric bay come out alike."""
    b = c.box
    return c.loc[2] - b.frm[2] if c.n[0] < 0 else b.to[2] - c.loc[2]


def shade(p, d):
    """A livery texel or a fixed colour, lighter (d > 0) or darker."""
    if isinstance(p, Paint):
        return Paint(p.slot, p.k + d)
    return mix(p, (255, 255, 255), d) if d > 0 else mix(p, (0, 0, 0), -d)


def pane(y, u, v, top, bottom, at=6):
    """Tinted glass, lighter towards the top, with one glint of sky near the pane's upper
    left corner: u and v count texels from that corner, rightwards and down."""
    c = mix(GLASS_DARK, GLASS_MID, (y - bottom) / max(1.0, top - bottom))
    s = u + v
    if 1 <= v <= 5 and s in (at, at + 1):
        return GLASS_HI
    if 2 <= v <= 4 and s == at + 4:
        return GLASS_LIGHT
    return c


# ---------------------------------------------------------------- displays and lamps

FONT = {
    "1": ("01", "11", "01", "01", "01"),
    "2": ("111", "001", "111", "100", "111"),
    "3": ("111", "001", "011", "001", "111"),
    "A": ("010", "101", "111", "101", "101"),
    "K": ("101", "101", "110", "101", "101"),
    "O": ("010", "101", "101", "101", "010"),
    "P": ("110", "101", "110", "100", "100"),
    "R": ("110", "101", "110", "101", "101"),
    "Z": ("111", "001", "010", "100", "111"),
}


def text_columns(text):
    out = []
    for ch in text:
        g = FONT[ch]
        for k in range(len(g[0])):
            out.append([g[r][k] == "1" for r in range(5)])
        out.append([False] * 5)
    return out[:-1]


def display_paint(line, dest):
    """A dot-matrix display: the line number on the left, the destination after it."""
    num = text_columns(line)
    word = text_columns(dest)

    def paint(c):
        b = c.box
        if not c.front:
            return SIGN_BG
        col = fl(b.to[0] - c.loc[0])  # from the viewer's left
        row = fl(b.to[1] - c.loc[1])  # from the top
        rows = cols(b.to[1] - b.frm[1])
        row -= (rows - 5) // 2
        if not 0 <= row < 5:
            return SIGN_BG
        k = col - 1
        if 0 <= k < len(num):
            return SIGN_TXT if num[k][row] else SIGN_BG
        k = col - (len(num) + 3)
        if 0 <= k < len(word):
            return SIGN_TXT if word[k][row] else SIGN_BG
        return SIGN_BG
    return paint


def side_sign_paint(line):
    digits = text_columns(line)

    def paint(c):
        if not c.side:
            return SIGN_BG
        u = fl(from_left(c))
        row = fl(c.box.to[1] - c.loc[1]) - (cols(size(c.box)[1]) - 5) // 2
        k = u - 1
        if 0 <= k < len(digits) and 0 <= row < 5:
            return SIGN_TXT if digits[k][row] else SIGN_BG
        return SIGN_BG
    return paint


def lamp_paint(kind):
    """Lamps painted from their inner edge, so a pair comes out mirrored."""
    def paint(c):
        if not (c.front or c.back):
            return HOUSING_DARK
        b = c.box
        d = fl(abs(c.x) - min(abs(b.frm[0]), abs(b.to[0])))
        iy = fl(c.loc[1] - b.frm[1])
        h = cols(size(b)[1])
        if kind == "head":
            # The daylight strip over the lens.
            if iy == h - 1:
                return DRL
            if d == 0:
                return HOUSING
            return LENS if iy == 0 else DRL
        return TAIL_RED if iy == 0 else TAIL_BRIGHT
    return paint


# ---------------------------------------------------------------- shared parts


def paint_bellows(c):
    x, y, z = c.p
    if c.side or c.top:
        return BELLOWS_HI if fl(z) % 2 == 0 else BELLOWS
    if c.bottom:
        return BELLOWS
    # The open end: the bellows' frame round the dark gangway.
    b = c.box
    ax = abs(x)
    hx = b.to[0]
    if ax > hx - 2 or y > b.to[1] - 2:
        return BELLOWS_HI if ax > hx - 1 or y > b.to[1] - 1 else BELLOWS
    if y < b.frm[1] + 1.5:
        return FRAME_HI  # the floor plate
    return hexc("#0f1011")


def paint_frame(c):
    if c.top:
        return FRAME_HI
    if c.side and fl(c.y) == fl(c.box.to[1] - 1e-3):
        return FRAME_HI
    return FRAME if c.side else FRAME_LO


def paint_ac(c):
    lx, ly, lz = rel(c)
    sx, sy, sz = size(c.box)
    if c.top:
        # Two condenser fans under grilles.
        for fz in (sz * 0.28, sz * 0.72):
            d = math.hypot(lx - sx / 2, lz - fz)
            if d < 5.6:
                if d < 1.2:
                    return EQUIP_HI
                if d > 4.6:
                    return Paint("roof", -0.3)
                a = math.degrees(math.atan2(lz - fz, lx - sx / 2)) % 90
                return VENT if a < 30 else EQUIP
        if fl(lx) in (0, cols(sx) - 1) or fl(lz) in (0, cols(sz) - 1):
            return Paint("roof", 0.16)
        return Paint("roof", 0.02)
    if c.bottom:
        return UNDER
    if c.side:
        iy = fl(ly)
        if iy == 0:
            return Paint("roof", -0.3)
        if iy < cols(sy) - 1 and 2 <= fl(lz) < cols(sz) - 2:
            return VENT if fl(lz) % 2 == 0 else Paint("roof", -0.16)
        return Paint("roof", -0.04)
    return Paint("roof", -0.1)


# ---------------------------------------------------------------- the tram

T_HW = 19.2        # 2.4 m wide
T_BOT = 4.0        # the skirt's lower edge, 0.25 m over the rail
T_TOP = 52.8       # 3.3 m
T_ROOF = 54.4      # the roof skin
T_FAIR = 58.4      # the fairings that hide the roof equipment
T_FAIR_HW = 17.2
T_DOOR_TOP = 46    # 2.87 m
T_GLASS = (21, 42)  # glass rows; the frame is the row under and over
T_HOPPER = 37      # the rail under the top-hung vent of each window
T_STRIPE = (16, 17)  # the waist line, in the livery's stripe colour

# The windscreen is one raked slab from the nose at (y, z) = (20, -59.2) up to the roof,
# the destination display behind its upper part.
T_WS_LEN = 34.0
T_WS_RISE = T_TOP - 20.0
T_WS_ANGLE = math.degrees(math.acos(T_WS_RISE / T_WS_LEN))
T_WS_TOP_Z = -59.2 + T_WS_LEN * math.sin(math.radians(T_WS_ANGLE))  # about -50.25
T_WS_GLASS = 24  # glass texel rows up the slab, then the black visor


def t_wall(iy):
    """A tram's side where there is no window or door, by texel row."""
    if iy <= 4:
        return Paint("lower", -0.42)
    if iy <= 12:
        return Paint("lower", {5: -0.2, 6: -0.1, 12: 0.16}.get(iy, 0.0))
    if iy == 13:
        return Paint("body", -0.22)
    if iy in T_STRIPE:
        return Paint("stripe", 0.08 if iy == T_STRIPE[1] else -0.04)
    if iy >= 52:
        return Paint("body", 0.24)
    if iy == 51:
        return Paint("body", 0.1)
    return Paint("body", 0.0)


def t_pane(iy, a, n, u):
    """A tram window in a bay of n columns (a from the bay's -z end, u from the left as
    seen): a pillar either side, a top-hung vent over the big pane."""
    lo, hi = T_GLASS
    if a <= 0 or a >= n - 1:
        return GLOSS
    if iy in (lo - 1, hi + 1):
        return TRIM
    if a in (1, n - 2) and iy in (lo, hi, T_HOPPER - 1, T_HOPPER + 1):
        return TRIM
    if iy == T_HOPPER:
        return TRIM
    if iy > T_HOPPER:
        return pane(iy + 0.5, u - 1, 9, hi + 1, lo)
    return pane(iy + 0.5, u - 1, T_HOPPER - 1 - iy, hi + 1, lo)


def t_door(iy, a, n, u):
    """A double plug door with tall glass: frame gap, two leaves, a rubber seam."""
    if iy >= T_DOOR_TOP:
        return Paint("body", -0.4) if iy == T_DOOR_TOP else t_wall(iy)
    if a <= 0 or a >= n - 1:
        return shade(t_wall(iy), -0.42) if iy >= 5 else TRIM
    if iy <= 4:
        return TRIM
    mid = n // 2
    if a in (mid - 1, mid):
        return GLOSS
    g = a - 1 if a < mid else n - 2 - a  # columns from the leaf's outer edge
    width = mid - 2
    if g == 0:
        return Paint("door", 0.12)
    if iy == T_DOOR_TOP - 1:
        return Paint("door", 0.16)
    if 9 <= iy <= 42 and 1 <= g <= width - 1:
        if iy in (9, 42) and g in (1, width - 1):
            return Paint("door", -0.06)
        if iy == 24:
            return Paint("door", 0.04)
        uu = (u - 2) if u < mid else (u - mid - 1)
        if iy > 24:
            return pane(iy + 0.5, uu, 42 - iy, 43, 9, at=3)
        return pane(iy + 0.5, uu, 23 - iy, 43, 9, at=3)
    if iy == 22 and g == width:
        return BUTTON
    return Paint("door", -0.1 if iy <= 8 else 0.0)


def t_bay_side(c):
    kind = c.box.extra["kind"]
    _, _, lz = rel(c)
    n = cols(size(c.box)[2])
    a = fl(lz)
    u = fl(from_left(c))
    iy = fl(c.y)
    lo, hi = T_GLASS
    if kind == "door":
        return t_door(iy, a, n, u)
    if kind == "cab":
        # The driver's side window runs on from the corner glass; a slider in it.
        if lo - 1 <= iy <= hi + 1:
            if a >= n - 2:
                return GLOSS
            if iy in (lo - 1, hi + 1) or a == 9:
                return TRIM
            if a == n - 3 and iy in (lo, hi):
                return TRIM
            return pane(iy + 0.5, a - 10 if a > 9 else a + 4, hi - iy, hi + 1, lo)
        return t_wall(iy)
    if lo - 1 <= iy <= hi + 1:
        return t_pane(iy, a, n, u)
    return t_wall(iy)


def t_end_face(c):
    """The body's end beside the bellows: the bands carried round the corner."""
    iy = fl(c.y)
    lo, hi = T_GLASS
    if lo - 1 <= iy <= hi + 1:
        return GLOSS
    return shade(t_wall(iy), -0.12)


def paint_t_bay(c):
    if c.bottom:
        return UNDER
    if c.top:
        return Paint("body", 0.2)
    if c.side:
        return t_bay_side(c)
    return t_end_face(c)


def paint_t_nose(c):
    """The lower front: the lamp band in black, the body colour round it."""
    x, y, z = c.p
    iy = fl(y)
    if c.bottom:
        return UNDER
    if c.top:
        return Paint("body", 0.22)
    if c.side:
        return t_wall(iy)
    ax = abs(x)
    if iy <= 12:
        return t_wall(iy)
    if iy == 13:
        return Paint("body", -0.12)
    if iy == 19:
        return Paint("body", 0.2)
    if 14 <= iy <= 17 and 5.5 <= ax < 17.5:
        return GLOSS
    if iy in T_STRIPE:
        return Paint("stripe", 0.12 if iy == T_STRIPE[1] else 0.0)
    return Paint("body", 0.06)


def paint_t_bumper(c):
    x, y, z = c.p
    iy = fl(y)
    if c.bottom:
        return UNDER
    if c.top:
        return Paint("lower", 0.22)
    if c.front:
        ax = abs(x)
        if iy == 12:
            return Paint("lower", 0.18)
        if iy == 3:
            return Paint("lower", -0.4)
        # The coupler cover.
        if ax < 6.5 and 5 <= iy <= 10:
            if fl(ax) == 6 or iy in (5, 10):
                return Paint("lower", -0.36)
            return Paint("lower", 0.06)
        return Paint("lower", 0.0)
    if iy == 3:
        return Paint("lower", -0.4)
    return Paint("lower", 0.12 if iy == 12 else -0.08)


def paint_t_windscreen(c):
    """The raked front: glass up to the waist of the hood, black over it; the edges and
    the glass round the corner are along the slab's own axes."""
    lx, ly, lz = rel(c)
    sx, sy, sz = size(c.box)
    if c.face == "north":
        w = cols(sx)
        u = fl(c.box.to[0] - c.loc[0])  # from the left as seen from ahead
        b = fl(ly)
        if b >= T_WS_GLASS + 1:
            return Paint("body", 0.24) if b >= cols(sy) - 1 else GLOSS
        if u in (0, w - 1) or b in (0, T_WS_GLASS):
            return GLOSS
        if u in (1, w - 2) and b in (1, T_WS_GLASS - 1):
            return GLOSS
        # The wiper, parked upright in the middle.
        if u == w // 2 and 1 <= b <= 8:
            return WIPER
        v = T_WS_GLASS - 1 - b
        if u >= w // 2 + 2:
            return pane(c.y, u - w // 2 - 2, v, 44, 21, at=7)
        return pane(c.y, u - 2, v, 44, 21, at=7)
    if c.side:
        a, b = fl(lz), fl(ly)
        if b >= T_WS_GLASS + 1:
            return Paint("body", 0.24) if b >= cols(sy) - 1 else GLOSS
        if a == 0 or b in (0, T_WS_GLASS):
            return GLOSS
        return pane(c.y, a + 2, T_WS_GLASS - 1 - b, 44, 21)
    if c.top:
        return Paint("body", 0.24)
    return GLOSS


def paint_t_roof(c):
    x, y, z = c.p
    if c.top:
        if abs(x) > 17.6:
            return Paint("roof", 0.22)
        if fl(z + 200) % 16 == 0:
            return Paint("roof", -0.1)
        return Paint("roof", 0.06)
    if c.bottom:
        return UNDER
    if c.side:
        return Paint("roof", -0.22) if fl(y) == 52 else Paint("roof", 0.04)
    return Paint("roof", -0.06) if c.front else Paint("roof", -0.14)


def paint_t_fairing(c):
    """The fairing over the roof equipment, in the body colour, louvres along its foot;
    the hatches on top are the roof colour."""
    x, y, z = c.p
    lx, ly, lz = rel(c)
    if c.top:
        ax = abs(x)
        edge = c.box.to[0] - 1
        if ax > edge:
            return Paint("body", 0.18)
        u = fl(z + 200) % 24
        if ax > edge - 2:
            return Paint("roof", -0.18)
        if u in (0, 23):
            return Paint("roof", -0.2)
        return Paint("roof", 0.02)
    if c.bottom:
        return UNDER
    if c.side:
        iy = fl(y)
        if iy == fl(T_ROOF):
            return Paint("body", -0.3)
        if iy >= fl(T_FAIR - 1):
            return Paint("body", 0.2)
        u = fl(z + 200) % 32
        if 12 <= u <= 21 and iy in (fl(T_ROOF) + 1, fl(T_ROOF) + 2):
            if u in (12, 21):
                return Paint("body", -0.2)
            return VENT if u % 2 == 1 else Paint("body", -0.28)
        return Paint("body", 0.02)
    if c.front:
        return Paint("body", 0.1)
    return Paint("body", -0.08)


def paint_t_slope(c):
    """The sloped end of a fairing."""
    if c.face == "up":
        lx, ly, lz = rel(c)
        if abs(c.x) > c.box.to[0] - 1:
            return Paint("body", 0.14)
        return Paint("body", 0.04)
    if c.side:
        return Paint("body", 0.02)
    return Paint("body", -0.1)


def paint_skirt(c):
    """The panel over a bogie, a little proud of the body, its edges a panel gap."""
    iy = fl(c.y)
    _, _, lz = rel(c)
    n = cols(size(c.box)[2])
    a = fl(lz)
    if c.side:
        if a in (0, n - 1) or iy == fl(c.box.to[1] - 1):
            return Paint("lower", -0.38)
        if iy == fl(c.box.frm[1]):
            return Paint("lower", -0.46)
        return Paint("lower", 0.16 if iy == fl(c.box.to[1] - 2) else 0.02)
    if c.top:
        return Paint("lower", 0.2)
    return Paint("lower", -0.3)


def paint_t_wheel(c):
    """A tram wheel's lower part under the skirt: the tyre and the rim's edge."""
    if c.side:
        iy = fl(c.y)
        return TREAD if iy == 0 else (WHEEL if iy == 1 else WHEEL_DISC)
    return TREAD if c.bottom else WHEEL


def paint_mirror(c):
    if c.back:
        return MIRROR_GLASS
    if c.top:
        return FRAME_HI
    return FRAME


def paint_panto(c):
    if c.top:
        return hexc("#8c939c")
    if c.bottom:
        return hexc("#3d4148")
    return STEEL


def paint_panto_head(c):
    if c.top:
        return CARBON if abs(c.x) < c.box.to[0] - 2 else STEEL
    return hexc("#4a4e55")


def paint_insulator(c):
    if c.top or c.bottom:
        return hexc("#8c8576")
    return INSULATOR if fl(c.y) % 2 == 0 else hexc("#a59d8c")


def tram_painters():
    return {
        "bay": paint_t_bay,
        "nose": paint_t_nose,
        "bumper": paint_t_bumper,
        "windscreen": paint_t_windscreen,
        "roof": paint_t_roof,
        "fairing": paint_t_fairing,
        "slope": paint_t_slope,
        "ac": paint_ac,
        "bellows": paint_bellows,
        "skirt": paint_skirt,
        "frame": paint_frame,
        "wheel": paint_t_wheel,
        "under": lambda c: UNDER,
        "display": display_paint("12", "ZOO"),
        "side_sign": side_sign_paint("12"),
        "head": lamp_paint("head"),
        "tail": lamp_paint("tail"),
        "mirror": paint_mirror,
        "panto": paint_panto,
        "panto_head": paint_panto_head,
        "insulator": paint_insulator,
    }


def t_bays(bays):
    return [Box(f"bay_{k}_{kind}", (-T_HW, T_BOT, z0), (T_HW, T_TOP, z1), "bay", kind=kind)
            for k, (z0, z1, kind) in enumerate(bays)]


def t_bellows(name, end, half):
    """Bellows at the end `end` (-1 or 1): 2 units into the body, 3.2 out of it."""
    z0, z1 = sorted((end * (half - 2.0), end * (half + 3.2)))
    return Box(name, (-17.6, 6.0, z0), (17.6, 51.0, z1), "bellows")


def t_fairing(prefix, z0, z1, slope_front=True, slope_back=True):
    """A fairing from z0 to z1 on the roof, its ends sloped at 30 degrees: a slab whose
    top face is the slope, a little narrower than the fairing so the sides do not meet."""
    run = 8.0 * math.cos(math.radians(30))  # how far a slope reaches back, rising 4
    out = []
    a = z0 + run if slope_front else z0
    b = z1 - run if slope_back else z1
    out.append(Box(f"{prefix}", (-T_FAIR_HW, T_ROOF, a), (T_FAIR_HW, T_FAIR, b), "fairing"))
    hw = T_FAIR_HW - 0.25
    if slope_front:
        out.append(Box(f"{prefix}_slope_f", (-hw, T_ROOF - 4.0, z0), (hw, T_ROOF, z0 + 8.0), "slope",
                       origin=(0, T_ROOF, z0), rot=(-30, 0, 0)))
    if slope_back:
        out.append(Box(f"{prefix}_slope_r", (-hw, T_ROOF - 4.0, z1 - 8.0), (hw, T_ROOF, z1), "slope",
                       origin=(0, T_ROOF, z1), rot=(30, 0, 0)))
    return out


def t_bogie(prefix, bz):
    """A bogie under the skirt: the frame and wheels show only under the body."""
    out = []
    out += pair(Box(f"{prefix}_frame_right", (13.8, 1.6, bz - 18), (16.4, T_BOT, bz + 18), "frame"))
    for k, wz in enumerate((bz - 14.4, bz + 14.4)):
        out += pair(Box(f"{prefix}_wheel{k}_right", (10.6, 1.4, wz - 5), (13.4, T_BOT, wz + 5), "wheel"))
        out += pair(Box(f"{prefix}_wheel{k}_crown_right", (10.3, 0, wz - 3.5), (13.7, T_BOT, wz + 3.5), "wheel"))
    out += pair(Box(f"{prefix}_skirt_right", (T_HW, 2.6, bz - 21), (T_HW + 0.3, 13.0, bz + 21), "skirt"))
    return out


def t_under(name, z0, z1):
    """The shadow under the floor, so the body does not float."""
    return Box(name, (-9.5, 2.4, z0), (9.5, T_BOT, z1), "under")


def tram_cab():
    half = 3.7 * M  # 59.2
    face = -half
    zb = -50.2  # where the body bays start, just behind the windscreen's top edge
    boxes = t_bays([(zb, -30.8, "cab"), (-30.8, -8.8, "win"), (-8.8, 13.2, "win"),
                    (13.2, 37.2, "door"), (37.2, half, "win")])
    add = boxes.append
    # The nose: the lower front with the lamps, the bumper in the lower colour, and the
    # raked windscreen up to the roof with the destination display behind its top.
    add(Box("nose", (-T_HW, T_BOT, face), (T_HW, 20.0, zb), "nose"))
    add(Box("bumper", (-17.6, 3.0, face - 2.0), (17.6, 13.0, face), "bumper"))
    ws = dict(origin=(0, 20.0, face), rot=(T_WS_ANGLE, 0, 0))
    add(Box("windscreen", (-18.5, 20.0, face), (18.5, 20.0 + T_WS_LEN, face + 10.0), "windscreen", **ws))
    add(Box("display", (-12.0, 20.0 + T_WS_GLASS + 2.0, face - 0.3), (12.0, 20.0 + T_WS_GLASS + 7.0, face),
            "display", lamp="glow", **ws))
    boxes += pair(Box("head_right", (10.5, 14.0, face - 0.25), (16.5, 17.0, face), "head", lamp="head"))
    boxes += pair(Box("tail_right", (6.5, 14.0, face - 0.25), (9.0, 17.0, face), "tail", lamp="glow"))
    boxes += pair(Box("mirror_arm_right", (T_HW, 41.0, -50.1), (21.4, 42.0, -49.4), "mirror"))
    boxes += pair(Box("mirror_right", (21.0, 35.0, -50.5), (22.6, 43.0, -49.0), "mirror"))
    # Roof: the skin, the fairing over the equipment and the air conditioning on it.
    add(Box("roof", (-18.6, T_TOP, zb), (18.6, T_ROOF, half), "roof"))
    boxes += t_fairing("fairing", -46.0, 50.0)
    add(Box("ac", (-9.0, T_FAIR, -32.0), (9.0, T_FAIR + 2.5, -6.0), "ac"))
    boxes += pair(Box("side_sign_right", (T_HW, 46.0, 21.2), (T_HW + 0.25, 51.0, 29.2), "side_sign", lamp="glow"))
    add(t_bellows("bellows", 1, half))
    boxes += t_bogie("bogie", -9.6)
    add(t_under("under", -48.0, 55.0))
    # train_bodies.rs doors(): riders every 3.7 m, eyes 2.12 m up; this is the one at the
    # door end (the other is as near the middle).
    return Model("train_tram_cab", boxes, tram_painters(), TRAM_LIVERY,
                 eye=(0.0, RIDER_EYE, 1.85 * M),
                 notes="7.4 m cab section; cab at -z")


def pantograph(prefix, pz, base):
    """A single-arm pantograph standing on `base`, centred on z = `pz`: the frame on
    insulators, the lower arm rising forward to the knee, the upper arm back to the head."""
    out = []
    out += pair(Box(f"{prefix}_insulator_f_right", (5.0, base, pz - 7.0), (7.0, base + 2.0, pz - 5.0), "insulator"))
    out += pair(Box(f"{prefix}_insulator_r_right", (5.0, base, pz + 5.0), (7.0, base + 2.0, pz + 7.0), "insulator"))
    y0 = base + 2.0
    out.append(Box(f"{prefix}_frame", (-7.5, y0, pz - 8.0), (7.5, y0 + 1.0, pz + 8.0), "panto"))
    hinge = (y0 + 1.0, pz + 6.0)
    knee = (y0 + 9.5, pz - 9.0)
    la = math.degrees(math.atan2(knee[0] - hinge[0], hinge[1] - knee[1]))
    ll = math.hypot(knee[0] - hinge[0], hinge[1] - knee[1])
    out.append(Box(f"{prefix}_lower_arm", (-3.0, hinge[0], hinge[1] - ll), (3.0, hinge[0] + 1.0, hinge[1]),
                   "panto", origin=(0, hinge[0], hinge[1]), rot=(la, 0, 0)))
    head = (y0 + 14.0, pz + 1.0)
    ua = math.degrees(math.atan2(head[0] - knee[0], head[1] - knee[1]))
    ul = math.hypot(head[0] - knee[0], head[1] - knee[1])
    out.append(Box(f"{prefix}_upper_arm", (-0.6, knee[0], knee[1]), (0.6, knee[0] + 1.0, knee[1] + ul),
                   "panto", origin=(0, knee[0], knee[1]), rot=(-ua, 0, 0)))
    out.append(Box(f"{prefix}_head", (-12.0, head[0], head[1] - 1.5), (12.0, head[0] + 1.0, head[1] + 1.5), "panto_head"))
    out += pair(Box(f"{prefix}_horn_right", (12.0, head[0] - 1.0, head[1] - 1.0), (13.6, head[0] + 0.5, head[1] + 1.0), "panto_head"))
    return out


def tram_mid():
    half = 3.5 * M  # 56
    boxes = t_bays([(-half, -34.0, "win"), (-34.0, -12.0, "win"), (-12.0, 12.0, "door"),
                    (12.0, 34.0, "win"), (34.0, half, "win")])
    add = boxes.append
    add(Box("roof", (-18.6, T_TOP, -half), (18.6, T_ROOF, half), "roof"))
    boxes += t_fairing("fairing", -48.0, 48.0)
    # train_bodies.rs: the pantograph 1.8 m behind the middle (Rust z -1.8).
    boxes += pantograph("panto", 1.8 * M, T_FAIR)
    boxes += pair(Box("side_sign_right", (T_HW, 46.0, -4.0), (T_HW + 0.25, 51.0, 4.0), "side_sign", lamp="glow"))
    add(t_bellows("bellows_f", -1, half))
    add(t_bellows("bellows_r", 1, half))
    add(t_under("under", -54.0, 54.0))
    return Model("train_tram_mid", boxes, tram_painters(), TRAM_LIVERY,
                 eye=(0.0, RIDER_EYE, 1.75 * M),
                 notes="7.0 m middle section, door and pantograph, hangs between the short sections")


def tram_short():
    half = 2.0 * M  # 32
    boxes = t_bays([(-half, -10.0, "win"), (-10.0, 10.0, "win"), (10.0, half, "win")])
    add = boxes.append
    add(Box("roof", (-18.6, T_TOP, -half), (18.6, T_ROOF, half), "roof"))
    add(t_bellows("bellows_f", -1, half))
    add(t_bellows("bellows_r", 1, half))
    boxes += t_bogie("bogie", 0.0)
    add(t_under("under", -30.0, 30.0))
    return Model("train_tram_short", boxes, tram_painters(), TRAM_LIVERY,
                 eye=(0.0, RIDER_EYE, 0.0),
                 notes="4.0 m short section on a bogie, no doors")


# ---------------------------------------------------------------- the metro

H_HW = 21.2       # 2.65 m wide
H_BOT = 14.0      # the body's lower edge, just under the 0.95 m floor
H_TOP = 56.0      # 3.5 m
H_ROOF = 57.4
H_ROOF2 = 58.8
H_GLASS = (30, 44)  # glass rows; frame rows under and over
H_DOOR_TOP = 48
H_WHEEL_R = 6.5


def h_wall(iy):
    if iy <= 14:
        return Paint("lower", -0.32)
    if iy <= 19:
        return Paint("lower", {15: -0.14, 19: 0.14}.get(iy, 0.0))
    if iy == 20:
        return Paint("body", -0.24)
    if iy in (25, 26):
        return Paint("stripe", 0.08 if iy == 26 else -0.04)
    if iy >= 55:
        return Paint("body", 0.24)
    if iy == 54:
        return Paint("body", 0.1)
    return Paint("body", 0.0)


def h_window(iy, a, n, u):
    lo, hi = H_GLASS
    if a <= 1 or a >= n - 2 or not lo - 1 <= iy <= hi + 1:
        return h_wall(iy)
    if iy in (lo - 1, hi + 1) or a in (2, n - 3):
        return TRIM
    if a in (3, n - 4) and iy in (lo, hi):
        return TRIM
    return pane(iy + 0.5, u - 3, hi - iy, hi + 1, lo, at=7)


def h_door(iy, a, n, u):
    """A sliding double door with narrow, tall windows."""
    if iy >= H_DOOR_TOP:
        return Paint("body", -0.42) if iy == H_DOOR_TOP else h_wall(iy)
    if a <= 0 or a >= n - 1:
        return shade(h_wall(iy), -0.42) if iy > 14 else TRIM
    if iy <= 14:
        return TRIM
    mid = n // 2
    if a in (mid - 1, mid):
        return GLOSS
    g = a - 1 if a < mid else n - 2 - a
    width = mid - 2
    if iy == H_DOOR_TOP - 1:
        return Paint("door", 0.16)
    if 26 <= iy <= 44 and 2 <= g <= width - 2:
        if iy in (26, 44) and g in (2, width - 2):
            return Paint("door", -0.1)
        uu = (u - 3) if u < mid else (u - mid - 2)
        return pane(iy + 0.5, uu, 44 - iy, 45, 26, at=3)
    if g == 0:
        return Paint("door", 0.1)
    return Paint("door", -0.08 if iy <= 17 else 0.0)


def h_bay_side(c):
    kind = c.box.extra["kind"]
    _, _, lz = rel(c)
    n = cols(size(c.box)[2])
    a = fl(lz)
    u = fl(from_left(c))
    iy = fl(c.y)
    if kind == "door":
        return h_door(iy, a, n, u)
    if kind == "win":
        return h_window(iy, a, n, u)
    if kind == "end":
        if a == n // 2 and 22 <= iy <= 36:
            return CHROME_DARK  # a grab handle
        return h_wall(iy)
    # The cab: the black mask wrapping round the corner, the driver's window and the
    # cab door behind it.
    lo, hi = H_GLASS
    if a <= 1 and 28 <= iy <= 49:
        return GLOSS
    if 3 <= a <= 13:
        if lo - 1 <= iy <= hi + 1:
            if iy in (lo - 1, hi + 1) or a in (3, 13):
                return TRIM
            if a == 8:
                return TRIM
            return pane(iy + 0.5, (a - 4) if a < 8 else (a - 9), hi - iy, hi + 1, lo, at=3)
        return h_wall(iy)
    if 16 <= a <= 27:
        if iy >= H_DOOR_TOP:
            return Paint("body", -0.42) if iy == H_DOOR_TOP else h_wall(iy)
        if a in (16, 27):
            return shade(h_wall(iy), -0.42) if iy > 14 else TRIM
        if iy <= 14:
            return TRIM
        if 31 <= iy <= 43 and 18 <= a <= 25:
            if iy in (31, 43) and a in (18, 25):
                return Paint("door", -0.1)
            return pane(iy + 0.5, a - 18, 43 - iy, 44, 31, at=4)
        if iy == 27 and a == 25:
            return CHROME_DARK  # handle
        return Paint("door", 0.1 if a == 17 else 0.0)
    return h_wall(iy)


def h_end_face(c):
    return shade(h_wall(fl(c.y)), -0.12)


def paint_h_bay(c):
    if c.bottom:
        return UNDER
    if c.top:
        return Paint("body", 0.2)
    if c.side:
        return h_bay_side(c)
    if c.front:
        return paint_h_windscreen(c)
    return h_end_face(c)


def paint_h_windscreen(c):
    """The cab bay's front, seen through the opening in the mask: two panes."""
    x, y, z = c.p
    ix = fl(abs(x))
    iy = fl(y)
    if ix == 0 or iy in (28, 49) or ix >= 17:
        return GLOSS
    if ix == 1 or ix == 16:
        return TRIM
    u = fl(17 - x) if x > 0 else fl(-1 - x)  # from each pane's left edge as seen
    return pane(y, u - 1, 48 - iy, 49, 29, at=8)


def paint_h_mask(c):
    """The cab front round the windscreen: black over the waist, body colour under it."""
    x, y, z = c.p
    iy = fl(y)
    ax = abs(x)
    if c.bottom:
        return GLOSS if c.box.frm[1] > 40 else UNDER
    if c.top:
        return Paint("body", 0.2) if c.box.frm[1] > 40 else GLOSS
    if c.side:
        if ax < H_HW - 0.1:
            return TRIM  # the opening's reveal
        return GLOSS if 28 <= iy <= 49 else h_wall(iy)
    if iy >= 28:
        if iy >= 55:
            return Paint("body", 0.24)
        return GLOSS if ax < H_HW - 1 or iy <= 49 else Paint("body", 0.04)
    if iy == 27:
        return Paint("body", -0.2)
    if iy <= 14:
        return Paint("lower", -0.3)
    if iy <= 19:
        return Paint("lower", 0.06 if iy == 19 else -0.04)
    if iy == 20:
        return Paint("body", -0.24)
    if iy in (25, 26):
        return Paint("stripe", 0.08 if iy == 26 else -0.04)
    return Paint("body", 0.06)


def paint_h_roof(c):
    x, y, z = c.p
    if c.top:
        if abs(x) > c.box.to[0] - 1:
            return Paint("roof", 0.2)
        if fl(z + 256) % 32 == 0:
            return Paint("roof", -0.12)
        return Paint("roof", 0.04)
    if c.bottom:
        return UNDER
    if c.side:
        return Paint("roof", -0.24) if fl(y) == fl(c.box.frm[1]) else Paint("roof", 0.0)
    return Paint("roof", -0.12)


def paint_anticlimber(c):
    if c.front:
        return FRAME_HI if fl(c.y) % 2 == 0 else FRAME_LO
    if c.top:
        return FRAME_HI
    return FRAME


def paint_coupler(c):
    if c.front or c.back:
        x = abs(c.x)
        return CHROME_DARK if x < 1.5 and abs(c.y - 11) < 1.5 else FRAME
    return FRAME_HI if c.top else FRAME


def paint_equip(c):
    """Underfloor boxes: dark grey, a panel line every 12 units, louvres on some."""
    _, ly, lz = rel(c)
    if c.side:
        u = fl(lz)
        if u % 12 == 0:
            return VENT
        if fl(ly) == 0:
            return FRAME_LO
        if fl(ly) == cols(size(c.box)[1]) - 1:
            return EQUIP_HI
        if u % 12 in (4, 6, 8) and 1 <= fl(ly) <= 3:
            return VENT
        return EQUIP
    if c.top:
        return EQUIP
    return FRAME_LO if c.bottom else EQUIP


def paint_h_wheel(c):
    x, y, z = c.p
    wz, wy = c.box.extra["hub"]
    if c.side:
        d = math.hypot(z - wz, y - wy)
        if d < 1.6:
            return AXLE_CAP
        if d > H_WHEEL_R + 0.2:
            return FRAME_LO  # the corners of the boxes, outside the wheel
        if d > H_WHEEL_R - 1.0:
            return TREAD
        if d > H_WHEEL_R - 2.0:
            return WHEEL
        return WHEEL_DISC
    return TREAD if c.bottom else WHEEL


def paint_axlebox(c):
    """The axle box under its coil spring: the spring's turns are level lines."""
    _, ly, lz = rel(c)
    if c.side:
        iy, a = fl(ly), fl(lz)
        h, w = cols(size(c.box)[1]), cols(size(c.box)[2])
        if iy == h - 1:
            return FRAME_HI
        if iy >= h - 5:
            if a in (0, w - 1):
                return FRAME_LO
            return STEEL if iy % 2 == 0 else HOUSING_DARK
        if a in (0, w - 1) or iy == 0:
            return FRAME_LO
        if iy in (1, 2) and 2 <= a <= w - 3:
            return AXLE_CAP
        return FRAME
    return FRAME_HI if c.top else FRAME


def paint_vent(c):
    lx, ly, lz = rel(c)
    if c.top:
        sx, _, sz = size(c.box)
        if fl(lx) in (0, cols(sx) - 1) or fl(lz) in (0, cols(sz) - 1):
            return Paint("roof", 0.16)
        return VENT if fl(lz) % 2 == 1 else Paint("roof", -0.22)
    return Paint("roof", -0.08 if c.side else -0.16)


def paint_shoe(c):
    if c.top:
        return hexc("#5b5247")
    return SHOE if fl(c.y) % 2 == 0 else hexc("#3a332b")


def metro_painters():
    return {
        "bay": paint_h_bay,
        "mask": paint_h_mask,
        "roof": paint_h_roof,
        "ac": paint_ac,
        "bellows": paint_bellows,
        "frame": paint_frame,
        "axlebox": paint_axlebox,
        "wheel": paint_h_wheel,
        "shoe": paint_shoe,
        "vent": paint_vent,
        "equip": paint_equip,
        "anticlimber": paint_anticlimber,
        "coupler": paint_coupler,
        "under": lambda c: UNDER,
        "display": display_paint("3", "PARK"),
        "head": lamp_paint("head"),
        "tail": lamp_paint("tail"),
    }


def h_bogie(prefix, bz):
    """An outside-frame bogie with two axles, its wheels in their own bones, and the
    third-rail shoes."""
    out, bones = [], {}
    out += pair(Box(f"{prefix}_beam_right", (13.6, 6.0, bz - 20.0), (16.6, 10.0, bz + 20.0), "frame"))
    out.append(Box(f"{prefix}_bolster", (-13.6, 9.0, bz - 4.0), (13.6, 13.0, bz + 4.0), "frame"))
    for k, wz in enumerate((bz - 17.6, bz + 17.6)):
        bone = f"wheel_{prefix}_{k}"
        bones[bone] = (0.0, H_WHEEL_R, wz)
        r = H_WHEEL_R
        out += pair(Box(f"{bone}_tread_right", (10.0, r - 4.5, wz - r), (13.0, r + 4.5, wz + r), "wheel", bone, hub=(wz, r)))
        out += pair(Box(f"{bone}_crown_right", (9.7, 0.0, wz - 4.5), (13.3, 2 * r, wz + 4.5), "wheel", bone, hub=(wz, r)))
        out += pair(Box(f"{prefix}_axlebox{k}_right", (13.9, 3.5, wz - 3.5), (17.4, 11.5, wz + 3.5), "axlebox"))
    out += pair(Box(f"{prefix}_shoe_right", (16.6, 1.6, bz - 5.0), (20.6, 3.2, bz + 5.0), "shoe"))
    out += pair(Box(f"{prefix}_shoe_hanger_right", (16.6, 3.2, bz - 1.5), (17.6, 6.0, bz + 1.5), "frame"))
    return out, bones


def metro(cab):
    half = 8.0 * M  # 128
    front = -half
    bays = [(-120.0, -92.0, "win"), (-92.0, -68.0, "door"), (-68.0, -40.0, "win"),
            (-40.0, -12.0, "win"), (-12.0, 12.0, "door"), (12.0, 40.0, "win"),
            (40.0, 68.0, "win"), (68.0, 92.0, "door"), (92.0, 120.0, "win"),
            (120.0, half, "end")]
    if cab:
        bays = [(front + 0.5, -92.0, "cab")] + bays[1:]
    else:
        bays = [(-half, -120.0, "end")] + bays
    boxes = [Box(f"bay_{k}_{kind}", (-H_HW, H_BOT, z0), (H_HW, H_TOP, z1), "bay", kind=kind)
             for k, (z0, z1, kind) in enumerate(bays)]
    add = boxes.append
    bones = {}
    for name, bz in (("bogie_f", -89.6), ("bogie_r", 89.6)):
        parts, b = h_bogie(name, bz)
        boxes += parts
        bones.update(b)
    # Underfloor equipment between the bogies.
    for k, (z0, z1) in enumerate(((-62.0, -26.0), (-20.0, 10.0), (16.0, 40.0), (46.0, 62.0))):
        add(Box(f"equip_{k}", (-16.0, 8.0, z0), (16.0, H_BOT, z1), "equip"))
    # Roof in two steps, and the air conditioning (train_bodies.rs: 3.5 m ahead of the
    # middle, here -z).
    add(Box("roof", (-20.6, H_TOP, -half + 1.0), (20.6, H_ROOF, half - 1.0), "roof"))
    add(Box("roof_top", (-16.4, H_ROOF, -half + 2.0), (16.4, H_ROOF2, half - 2.0), "roof"))
    add(Box("ac", (-12.0, H_ROOF2, -3.5 * M - 20.8), (12.0, H_ROOF2 + 4.0, -3.5 * M + 20.8), "ac"))
    for k, vz in enumerate((-8.0, 40.0, 88.0)):
        add(Box(f"vent_{k}", (-5.0, H_ROOF2, vz - 6.0), (5.0, H_ROOF2 + 1.0, vz + 6.0), "vent"))
    ends = [1] if cab else [-1, 1]
    for end in ends:
        tag = "f" if end < 0 else "r"
        z0, z1 = sorted((end * (half - 1.0), end * (half + 3.2)))
        add(Box(f"gangway_{tag}", (-13.2, 16.0, z0), (13.2, 52.0, z1), "bellows"))
        z0, z1 = sorted((end * (half - 8.0), end * (half + 3.2)))
        add(Box(f"coupler_{tag}", (-2.5, 8.5, z0), (2.5, 12.5, z1), "coupler"))
    if cab:
        f = front
        # The mask round the windscreen, half a unit proud of the cab bay.
        add(Box("mask_low", (-H_HW, H_BOT, f), (H_HW, 28.0, f + 0.5), "mask"))
        add(Box("mask_top", (-H_HW, 50.0, f), (H_HW, H_TOP, f + 0.5), "mask"))
        boxes += pair(Box("mask_right", (17.5, 28.0, f), (H_HW, 50.0, f + 0.5), "mask"))
        add(Box("display", (-12.4, 50.5, f - 0.25), (12.4, 55.5, f), "display", lamp="glow"))
        boxes += pair(Box("head_right", (12.0, 21.0, f - 0.25), (18.0, 24.0, f), "head", lamp="head"))
        boxes += pair(Box("tail_right", (8.0, 21.0, f - 0.25), (10.5, 24.0, f), "tail", lamp="glow"))
        add(Box("anticlimber", (-15.0, 10.0, f - 3.0), (15.0, 16.0, f), "anticlimber"))
        add(Box("coupler_f", (-2.5, 9.0, f - 4.0), (2.5, 13.0, f - 3.0), "coupler"))
        add(Box("cab_skirt", (-19.0, 6.0, f + 0.5), (19.0, H_BOT, -112.0), "equip"))
    name = "train_metro_cab" if cab else "train_metro_mid"
    # train_bodies.rs doors(): riders at -5.33, 0 and 5.33 m, eyes 2.12 m up.
    return Model(name, boxes, metro_painters(), METRO_LIVERY, bones=bones,
                 eye=(0.0, RIDER_EYE, 0.0),
                 notes="16 m third-rail metro car" + (", cab at -z" if cab else ""))


def metro_cab():
    return metro(True)


def metro_mid():
    return metro(False)


MODELS = [tram_cab, tram_mid, tram_short, metro_cab, metro_mid]
