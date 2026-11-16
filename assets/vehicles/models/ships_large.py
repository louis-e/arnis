"""Large ships, 72 to 135 m: a double-ended car ferry and sea ferry, a Rhine motor cargo
ship as a dry-cargo, container and tank barge, and a river cruise ship. The barges and the
cruise ship come `_low` as well, for low bridges: wheelhouse let down, the top tier off.

Sizes, drafts, heights and seats follow ship_bodies.rs (its bow is at +z, ours at -z).
Long bodies are repeated segments painted relative to their own box, so each kind of
segment takes one spot of the texture. Hull: antifouling under water, a boot-top at the
waterline, the hull in the livery's `lower`, a sheer band in `stripe`, rub rails;
superstructure `body`, decks `door`, roofs and funnels `roof`."""
import math

from kit import (
    CHROME_DARK, GLASS_DARK, HOUSING, STEEL, TIRE, TRIM, UNDER, Box, Model, Paint, glass, hexc, mix,
)

# ---------------------------------------------------------------- colours

ANTIFOUL = hexc("#5c2b27")
ANTIFOUL_DARK = hexc("#45201d")
BOOT = hexc("#222327")
BOOT_HI = hexc("#34363b")
DECK = hexc("#5d6268")
DECK_HI = hexc("#6c7178")
DECK_LO = hexc("#4b4f55")
RUBBER = hexc("#1c1c1f")
RUBBER_HI = hexc("#2c2d31")
FRAME = hexc("#2a2d33")
WHITE = hexc("#e4e7ea")
WHITE_SH = hexc("#c3c8cd")
TANK_DECK = (146, 64, 48)      # the red tanker deck, as red_terracotta
TANK_DECK_HI = hexc("#a4513c")
TANK_DECK_LO = hexc("#7a3527")
PIPE = hexc("#b9bec4")
PIPE_HI = hexc("#d6dade")
PIPE_LO = hexc("#8d939a")
VALVE = hexc("#c4302b")
YELLOW = hexc("#e1b62c")
YELLOW_LO = hexc("#b18a1d")
ORANGE = hexc("#ea6a1f")
LAMP_RED = hexc("#ff3b3b")
LAMP_GREEN = hexc("#3dff7a")
LAMP_WHITE = hexc("#fffbea")
TEAK = hexc("#a87a4f")
TEAK_LO = hexc("#8d6440")

U = 16.0


def m(v):
    return v * U


# ---------------------------------------------------------------- building helpers


def flip_z(b, name):
    """The same box at the other end (z -> -z)."""
    return Box(
        name, (b.frm[0], b.frm[1], -b.to[2]), (b.to[0], b.to[1], -b.frm[2]), b.mat, b.bone,
        (b.origin[0], b.origin[1], -b.origin[2]), (-b.rot[0], -b.rot[1], b.rot[2]), b.lamp,
        **b.extra,
    )


class Ship:
    """Collects boxes, names each uniquely."""

    def __init__(self):
        self.boxes = []
        self.painters = {}
        self._k = 0

    def _name(self, base):
        self._k += 1
        return f"{base}_{self._k}"

    def box(self, base, frm, to, mat, **kw):
        b = Box(self._name(base), frm, to, mat, **kw)
        self.boxes.append(b)
        return b

    def take(self, b):
        b.name = self._name(b.name)
        self.boxes.append(b)
        return b

    def x2(self, base, frm, to, mat, **kw):
        """A box on the right (+x) and its mirror image on the left."""
        b = self.box(base, frm, to, mat, **kw)
        self.boxes.append(b.mirrored(self._name(base + "_l")))
        return b

    def z2(self, b):
        """Adds the end-for-end copy of a box already added."""
        self.boxes.append(flip_z(b, self._name(b.name + "_z")))

    def mirror_x(self, boxes):
        for b in boxes:
            self.boxes.append(b.mirrored(self._name(b.name + "_l")))

    def mirror_z(self, boxes):
        for b in boxes:
            self.boxes.append(flip_z(b, self._name(b.name + "_z")))


def rel(c):
    """The texel's spot from the box's own corner, before any turn."""
    return (c.loc[0] - c.box.frm[0], c.loc[1] - c.box.frm[1], c.loc[2] - c.box.frm[2])


def dims(c):
    return (c.box.to[0] - c.box.frm[0], c.box.to[1] - c.box.frm[1], c.box.to[2] - c.box.frm[2])


def outward(c):
    """Whether a side face looks away from the centreline."""
    return c.n[0] * (c.box.frm[0] + c.box.to[0]) > 0 or abs(c.box.frm[0] + c.box.to[0]) < 1e-6


# ---------------------------------------------------------------- hull


def hull_side(y, top):
    """The hull's side under the sheer band, whose foot is at `top`: antifouling, the
    boot-top at the waterline, the hull in `lower`, in the band's shadow at the top."""
    iy, it = math.floor(y), math.floor(top - 1e-6)
    if iy < -3:
        return ANTIFOUL
    if iy < 4:
        return BOOT
    if iy == 4:
        return BOOT_HI
    if iy == it:
        return Paint("lower", -0.35)
    if iy > it - 6:
        return Paint("lower", 0.04)
    if iy < 9:
        return Paint("lower", -0.1)
    return Paint("lower", -0.02)


def band_side(y, y0, top, stripe=5, rail=3):
    """The sheer band standing proud of the hull: a rub rail at its foot, the stripe on top."""
    iy, it, i0 = math.floor(y), math.floor(top - 1e-6), math.floor(y0)
    if iy > it - stripe:
        return Paint("stripe", 0.22 if iy == it else (-0.1 if iy == it - stripe + 1 else 0.02))
    if iy < i0 + rail:
        return Paint("lower", -0.18 if iy == i0 + rail - 1 else (-0.55 if iy == i0 else -0.38))
    return Paint("lower", 0.06)


class HullStyle:
    """How a hull is painted: the deck on its top (a painter of c), the bulwarks' inside,
    the sheer band's foot and stripe rows, white draft marks near the ends."""

    def __init__(self, deck, inner, band_y0, stripe=5, rail=3, marks=()):
        self.deck, self.inner, self.band_y0 = deck, inner, band_y0
        self.stripe, self.rail, self.marks = stripe, rail, marks

    def hull(self, c):
        """Hull boxes and plates: sides under the band, the deck or the bulwark's cap."""
        if c.bottom:
            return ANTIFOUL_DARK
        if c.top:
            return self.deck(c) if not any(c.box.rot) else self.inner(c, cap=True)
        if c.face in ("east", "west") and not outward(c):
            return self.inner(c)
        y = c.y
        if y >= self.band_y0:
            return band_side(y, self.band_y0, c.box.to[1], self.stripe, self.rail)
        if c.box.extra.get("anchor"):
            a = rel(c)[2] - dims(c)[2] / 2
            got = anchor_pocket(a if c.n[0] > 0 else -a, y - (self.band_y0 - 13))
            if got is not None:
                return got
        if self.marks and 5 <= y < self.band_y0 - 2:
            for z0, z1 in self.marks:
                if z0 <= c.z < z1 and math.floor(y) % 4 == 0:
                    return WHITE
        return hull_side(y, self.band_y0)

    def band(self, c):
        """The proud band: rub rail and stripe; its top a lit edge."""
        if c.bottom:
            return Paint("lower", -0.6)
        if c.top:
            return Paint("stripe", 0.25)
        if c.face in ("east", "west") and not outward(c) and not c.box.name.startswith("sheer"):
            return self.inner(c)
        return band_side(c.y, self.band_y0, c.box.to[1], self.stripe, self.rail)

    def sheer(self, c):
        """The midbody's sheer slab: the deck on top, the band on its sides."""
        if c.top:
            return self.deck(c)
        return self.band(c)

    def fill(self, c):
        if c.top:
            return self.deck(c)
        if c.bottom:
            return ANTIFOUL_DARK
        return self.inner(c)


def anchor_pocket(a, h):
    """An anchor stowed in its pocket: a is along the hull from the pocket's middle (towards
    the stern positive), h up from the pocket's foot; None outside it."""
    ia, ih = math.floor(a), math.floor(h)
    if not (-7 <= ia <= 6 and 0 <= ih <= 11):
        return None
    if ih == 11 or ia in (-7, 6):
        return Paint("lower", -0.55)
    if ih >= 9 and -2 <= ia <= 1:
        return hexc("#101012")
    if -1 <= ia <= 0 and 3 <= ih <= 8:
        return hexc("#7b8088")
    if ih == 7 and -3 <= ia <= 2:
        return hexc("#7b8088")
    if ih in (1, 2) and -5 <= ia <= 4:
        return hexc("#6b7078") if ih == 1 or abs(ia + 0.5) > 3 else hexc("#7b8088")
    if ih == 3 and ia in (-5, 4):
        return hexc("#6b7078")
    return Paint("lower", -0.45)


def plan_w(pts, d):
    """Half-width of a plan given as [(d, w), ...] from the shoulder towards the end."""
    for (da, wa), (db, wb) in zip(pts, pts[1:]):
        if db <= d <= da:
            return wb + (wa - wb) * (d - db) / (da - db)
    return pts[0][1] if d > pts[0][0] else pts[-1][1]


def hull_end(half, pts, tops, bots, t, rt, band_y0, fill_top, tip, pitch=8.0, name="bow",
             front_band=True, slab=6.0, anchor=None, reach=0.0, solid=False):
    """One end of a hull, at -z: side plates turned about y along the plan `pts`
    [(d from the end, plating half-width)], each with its top and bottom, the proud band
    along them, a deck slab of level steps between them and a plate across the tip,
    tip = (d of its front, half-width, top, bottom). Returns the boxes, both sides."""
    out = []
    angles = [math.degrees(math.atan2(wa - wb, da - db)) for (da, wa), (db, wb) in zip(pts, pts[1:])]
    assert all(0 <= a <= 45.0 + 1e-9 for a in angles), angles
    d_tip = pts[-1][0]
    tip_front, tip_w, tip_top, tip_bot = tip
    plate_bots = []
    for i, ((da, wa), (db, wb)) in enumerate(zip(pts, pts[1:])):
        za = -half + da
        ln = math.hypot(da - db, wa - wb)
        phi = angles[i]
        before = angles[i - 1] if i else 0.0
        last = i + 1 == len(angles)
        # Each plate runs on past its corner far enough to close the joint (none at the
        # shoulder, where the middle body closes it); bottoms step so none lie in another.
        e0 = 0.0 if i == 0 else t * math.tan(math.radians(abs(phi - before)) / 2) + 0.2
        e1 = (min(t, (d_tip - tip_front) / max(0.2, math.cos(math.radians(phi))) - 0.3) if last
              else t * math.tan(math.radians(abs(angles[i + 1] - phi)) / 2) + 0.2)
        bot = bots[i] - 0.5 + 0.3 * i
        plate_bots.append((da, db, bot))
        b = Box(f"{name}_plate", (wa - t, bot, za - ln - e1), (wa, tops[i], za + e0), "hull",
                origin=(wa, 0, za), rot=(0, phi, 0), anchor=(i == anchor))
        out += [b, b.mirrored(f"{name}_plate_l")]
        if rt > 0:
            f0 = 0.0 if i == 0 else rt * math.tan(math.radians(abs(phi - before)) / 2) + 0.1
            f1 = (0.0 if last else rt * math.tan(math.radians(abs(angles[i + 1] - phi)) / 2) + 0.1)
            r = Box(f"{name}_band", (wa, band_y0 + 0.3 * (i + 1), za - ln - f1), (wa + rt, tops[i], za + f0), "band",
                    origin=(wa, 0, za), rot=(0, phi, 0))
            out += [r, r.mirrored(f"{name}_band_l")]
    # The deck between the plates, in level steps a little narrower than the plates: each
    # as long as the plates' angle lets it stay inside them. `solid` steps reach down to
    # the plates' bottoms, so the middle body's end and the plates' insides are hidden.
    def pitch_at(d):
        for (da, wa), (db, wb), a in zip(pts, pts[1:], angles):
            if db - 1e-6 <= d <= da + 1e-6:
                return max(4.0, min(64.0, (t - 1.5) / max(0.01, math.sin(math.radians(a)))))
        return pitch

    d, d_end = tip_front + rt, pts[0][0] + reach
    while d < d_end - 1e-6:
        p = pitch_at(max(d, d_tip))
        p = min(p, pitch_at(min(d + p, d_end)))
        p = math.floor(p) if p > 8 else p
        d1 = d_end if d_end - (d + p) < 4 else min(d + p, d_end)
        w = (plan_w(pts, d) if d >= d_tip else tip_w) - 1.0
        if solid:
            under = [bt for (da, db, bt) in plate_bots if db - 1e-6 <= d1 and d <= da + 1e-6]
            y0 = max(under) + 0.3 if under else fill_top - slab
            if d < d_tip + 1:
                y0 = max(y0, min(tip_bot, bots[-1] - 1) + 0.3)
        else:
            y0 = fill_top - slab
        out.append(Box(f"{name}_deck", (-w, y0, -half + d), (w, fill_top, -half + d1), "fill"))
        d = d1
    tip_bot = min(tip_bot, bots[-1] - 1)
    out.append(Box(f"{name}_tip", (-tip_w, tip_bot, -half + tip_front + (rt if front_band else 0)),
                   (tip_w, tip_top, -half + d_tip + t), "hull"))
    if front_band and rt > 0:
        out.append(Box(f"{name}_band_front", (-tip_w - rt, band_y0, -half + tip_front),
                       (tip_w + rt, tip_top, -half + tip_front + rt), "band"))
    return out


def build_hull(s, style, L, HW, rt, bottom, deck, bow, stern, n_seg, double=False, strip_band=False):
    """A whole hull: the parallel middle body in n_seg identical segments (hull below the
    band, the sheer slab with the band and deck above) and the two ends. `bow` and
    `stern` are dicts of hull_end's arguments; a double-ended hull takes the bow twice."""
    half = L / 2
    s.painters["hull"] = style.hull
    s.painters["band"] = style.band
    s.painters["sheer"] = style.sheer
    s.painters["fill"] = style.fill
    w = HW - rt
    z0 = -half + bow["pts"][0][0]
    z1 = half - (bow if double else stern)["pts"][0][0]
    seg = (z1 - z0) / n_seg
    for k in range(n_seg):
        za, zb = z0 + k * seg, z0 + (k + 1) * seg
        if strip_band:
            # the band as strips on a hull reaching the deck, whose top something covers
            s.box("hull", (-w, bottom, za), (w, deck, zb), "hull")
            s.x2("band_strip", (w, style.band_y0, za), (HW, deck, zb), "band")
        else:
            s.box("hull", (-w, bottom, za), (w, style.band_y0, zb), "hull")
            s.box("sheer", (-HW, style.band_y0, za), (HW, deck, zb), "sheer")
    for b in hull_end(half, rt=rt, band_y0=style.band_y0, reach=1.0 if bow["fill_top"] > deck else 0.0, **bow):
        s.take(b)
    other = bow if double else stern
    for b in hull_end(half, rt=rt, band_y0=style.band_y0, name="stern",
                      reach=1.0 if other["fill_top"] > deck else 0.0, **other):
        s.take(flip_z(b, b.name))
    return z0, z1


# ---------------------------------------------------------------- common parts


def lamp_painter(c):
    if c.bottom:
        return HOUSING
    x = c.box.frm[0] + c.box.to[0]
    kind = c.box.extra.get("light", "side")
    if kind == "white":
        return LAMP_WHITE
    return LAMP_GREEN if x > 0 else LAMP_RED


def add_light(s, x, y, z, kind="side", size=2.0):
    """A navigation light: red to port (-x), green to starboard (+x), or white."""
    h = size / 2
    s.box("light", (x - h, y, z - h), (x + h, y + size, z + h), "lamp", lamp="glow", light=kind)


def paint_mast(c):
    if c.top:
        return Paint("roof", 0.1)
    iy = math.floor(c.y)
    return Paint("roof", -0.15 if c.side else 0.0) if iy % 12 else Paint("roof", -0.4)


def paint_steel(c):
    if c.top:
        return mix(STEEL, WHITE, 0.2)
    if c.bottom:
        return mix(STEEL, (0, 0, 0), 0.4)
    return STEEL if c.side else mix(STEEL, (0, 0, 0), 0.12)


def paint_dark(c):
    return RUBBER_HI if c.top else RUBBER


def railing(s, a, b, y, posts, h=16.0, mat="white_rail", ends=True, trim=0.0):
    """A railing from a to b, (x, z) along one axis: a top rail, a knee rail and posts;
    `trim` shortens the rails at both ends, to butt against a railing across."""
    (xa, za), (xb, zb) = a, b
    along_z = abs(zb - za) > abs(xb - xa)
    tz, tx = (trim, 0.0) if along_z else (0.0, trim)

    def span(w):
        return ((min(xa, xb) - w + tx, min(za, zb) - w + tz), (max(xa, xb) + w - tx, max(za, zb) + w - tz))
    (x0, z0), (x1, z1) = span(1.0)
    s.box("rail_top", (x0, y + h - 1.5, z0), (x1, y + h, z1), mat)
    (x0, z0), (x1, z1) = span(0.4)
    s.box("rail_knee", (x0, y + h / 2 - 0.5, z0), (x1, y + h / 2 + 0.5, z1), mat)
    k0, k1 = (0, posts) if ends else (1, posts - 1)
    for k in range(k0, k1 + 1):
        t = k / posts
        x, z = xa + (xb - xa) * t, za + (zb - za) * t
        s.box("rail_post", (x - 0.75, y, z - 0.75), (x + 0.75, y + h - 1.5, z + 0.75), mat)


def paint_white_rail(c):
    if c.top:
        return WHITE
    if c.bottom:
        return WHITE_SH
    return mix(WHITE, (0, 0, 0), 0.08)


# ---------------------------------------------------------------- cars


CAR_COLOURS = [hexc("#b8202a"), hexc("#e2e4e7"), hexc("#1f2125"), hexc("#2c4f9c"), hexc("#9ea4ab"),
               hexc("#4a4f57"), hexc("#7d1d24"), hexc("#d9d2c0")]


def car_painter(colour):
    def p(c):
        lz = rel(c)[2]
        ln = dims(c)[2]
        iy = math.floor(c.y - c.box.frm[1])
        if c.bottom:
            return UNDER
        if c.top:
            return mix(colour, (255, 255, 255), 0.12)
        if c.front or c.back:
            if iy in (8, 9):
                return LAMP_WHITE if c.front and abs(c.x - (c.box.frm[0] + c.box.to[0]) / 2) > 9 else (
                    hexc("#a3141f") if c.back and abs(c.x - (c.box.frm[0] + c.box.to[0]) / 2) > 9 else colour)
            if iy < 4:
                return TRIM
            return colour
        # side: wheel arches
        for wz in (ln * 0.2, ln * 0.8):
            if abs(lz - wz) < 5.5 and iy < 6:
                return TIRE if abs(lz - wz) < 4.5 and iy < 5 else TRIM
        if iy < 3:
            return TRIM
        if iy == 11:
            return mix(colour, (255, 255, 255), 0.15)
        return mix(colour, (0, 0, 0), 0.08 if iy < 6 else 0.0)
    return p


def car_cabin_painter(colour):
    def p(c):
        if c.top:
            return mix(colour, (0, 0, 0), 0.05)
        iy = math.floor(c.y - c.box.frm[1])
        if iy >= 7:
            return colour
        r = rel(c)
        if c.side:
            lz, ln = r[2], dims(c)[2]
            if abs(lz - ln * 0.55) < 1:
                return colour
            return glass(c.y, lz, iy, top=c.box.to[1], bottom=c.box.frm[1])
        return glass(c.y, r[0], iy, top=c.box.to[1], bottom=c.box.frm[1])
    return p


def add_car(s, x, y, z, k, length=70, facing=-1):
    """A car, its middle at (x, z), standing on y; colour k from CAR_COLOURS."""
    col = CAR_COLOURS[k % len(CAR_COLOURS)]
    name = f"car{k % len(CAR_COLOURS)}"
    s.painters.setdefault(name, car_painter(col))
    s.painters.setdefault(name + "_cab", car_cabin_painter(col))
    w = 14.5
    h = length / 2
    s.box("car", (x - w, y + 1, z - h), (x + w, y + 12, z + h), name)
    c0 = z - h * 0.55 if facing < 0 else z - h * 0.45
    s.box("car_cab", (x - w + 1.5, y + 12, c0), (x + w - 1.5, y + 21, c0 + h * 1.0), name + "_cab")


def add_truck(s, x, y, z, k, length=190):
    """A box lorry, cab towards -z."""
    col = CAR_COLOURS[k % len(CAR_COLOURS)]
    s.painters.setdefault("truck_box", paint_truck_box)
    s.painters.setdefault(f"truck_cab{k}", truck_cab_painter(col))
    h = length / 2
    s.box("truck_cab", (x - 19, y + 6, z - h), (x + 19, y + 48, z - h + 30), f"truck_cab{k}")
    s.box("truck_box", (x - 20, y + 10, z - h + 32), (x + 20, y + 64, z + h), "truck_box")
    s.box("truck_chassis", (x - 17, y + 0, z - h + 4), (x + 17, y + 10, z + h - 6), "dark")


def truck_cab_painter(col):
    def p(c):
        iy = math.floor(c.y - c.box.frm[1])
        if c.top:
            return mix(col, (255, 255, 255), 0.1)
        if iy >= 22 and iy < 38 and (c.front or c.side):
            r = rel(c)
            if c.side and r[2] > 22:
                return col
            return glass(c.y, r[0] if c.front else r[2], iy, top=c.box.frm[1] + 38, bottom=c.box.frm[1] + 22)
        if c.front and iy < 8:
            return TRIM if iy > 2 else UNDER
        if c.front and iy in (9, 10) and abs(c.x - (c.box.frm[0] + c.box.to[0]) / 2) > 13:
            return LAMP_WHITE
        return col if iy < 38 else mix(col, (255, 255, 255), 0.08)
    return p


def paint_truck_box(c):
    if c.top:
        return hexc("#d7dade")
    if c.bottom:
        return UNDER
    iy = math.floor(c.y - c.box.frm[1])
    if iy in (0, 53):
        return hexc("#8e949b")
    if c.back:
        ix = math.floor(c.x - c.box.frm[0])
        if ix in (19, 20):
            return hexc("#9ca2a9")
        return hexc("#c9cdd2")
    lz = math.floor(rel(c)[2])
    if lz % 24 == 0:
        return hexc("#c4c8cd")
    return hexc("#dfe2e5")


# ---------------------------------------------------------------- barges


BARGE_L, BARGE_HW = m(105.0), m(11.4) / 2
BARGE_RAIL = 2.2
BARGE_W = BARGE_HW - BARGE_RAIL          # the plating's half-width
BARGE_DOWN, BARGE_DECK = -45.0, 19.0
HOLD_FROM, HOLD_TO = -m(43.5), m(39.5)    # Rust: -half + 13 .. half - 9, ends swapped
ACC_FROM, ACC_TO = m(41.5), m(51.5)


def barge_hull(s, deck_paint):
    half = BARGE_L / 2
    style = HullStyle(
        deck=deck_paint,
        inner=lambda c, cap=False: Paint("body", 0.1) if cap else Paint("body", -0.12 if c.y > BARGE_DECK + 5 else -0.25),
        band_y0=10.0, marks=((-half + 40, -half + 44), (half - 44, half - 40)))
    bow = dict(pts=[(144.0, BARGE_W), (100, BARGE_W - 3), (62, BARGE_W - 16), (30, BARGE_W - 36), (12, BARGE_W - 52)],
               tops=[28, 29, 30, 31], bots=[-45, -43, -37, -28], t=8.0, fill_top=BARGE_DECK + 4,
               tip=(0.0, BARGE_W - 52, 33, -20), anchor=2)
    stern = dict(pts=[(96.0, BARGE_W), (46, BARGE_W - 1), (20, BARGE_W - 6), (8, BARGE_W - 16)],
                 tops=[24, 25, 26], bots=[-45, -36, -22], t=8.0, fill_top=BARGE_DECK + 1,
                 tip=(0.0, BARGE_W - 16, 28, -10))
    build_hull(s, style, BARGE_L, BARGE_HW, BARGE_RAIL, BARGE_DOWN, BARGE_DECK, bow, stern, 10)
    # Bollards in pairs along the side decks.
    s.painters["bollard"] = paint_bollard
    for z in (-m(46.5), -m(30), m(0), m(30), m(39.5)):
        s.x2("bollard", (BARGE_W - 9, BARGE_DECK, z), (BARGE_W - 4, BARGE_DECK + 6, z + 14), "bollard")


def paint_bollard(c):
    """A pair of bollards on one foot."""
    lz = rel(c)[2]
    iy = math.floor(c.y - c.box.frm[1])
    gap = 5 <= lz < 9
    if c.top:
        return hexc("#2a2c30") if gap else hexc("#44474d")
    if c.side and gap and iy > 0:
        return hexc("#151517")
    return hexc("#2f3236") if iy in (0, 4) else hexc("#25272b")


def gangboard(c):
    """The barge's side decks: steel, the band's lit edge outboard."""
    ax = abs(c.x)
    if ax > BARGE_W:
        return Paint("stripe", 0.25)
    if ax > BARGE_W - 1:
        return DECK_HI
    if c.box.name.startswith(("bow", "stern")):
        return DECK if math.floor(c.z) % 24 else DECK_LO
    return DECK if math.floor(rel(c)[2]) % 48 else DECK_LO


def paint_coaming(c):
    if c.top:
        return Paint("lower", -0.45)
    if c.bottom:
        return UNDER
    iy = math.floor(c.y)
    top = math.floor(c.box.to[1] - 1e-6)
    if iy == top:
        return Paint("lower", 0.18)
    if iy == top - 1:
        return Paint("lower", 0.06)
    if iy == math.floor(c.box.frm[1]):
        return Paint("lower", -0.3)
    if c.side:
        lz = math.floor(rel(c)[2])
        if lz in (0, 1):
            return Paint("lower", -0.18)
        if iy == top - 6 and 6 <= lz < dims(c)[2] - 6:
            return Paint("lower", -0.12)
    return Paint("lower", -0.02)


def paint_lid(c):
    """A hatch cover in the deck colour, its ridge lighter, its rim darker."""
    r, d = rel(c), dims(c)
    if c.bottom:
        return Paint("door", -0.6)
    if c.top:
        lx, lz = r[0], r[2]
        ix = math.floor(lx)
        mid = d[0] / 2
        if lz < 1 or lz > d[2] - 1:
            return Paint("door", -0.25)
        if lx < 1 or lx > d[0] - 1:
            return Paint("door", -0.2)
        if abs(lx - mid) < 1:
            return Paint("door", 0.2)
        if math.floor(lz) in (6, math.floor(d[2]) - 7) and abs(lx - mid) > 20 and abs(lx - mid) < 60:
            return Paint("door", -0.18)
        k = 0.08 - 0.12 * abs(lx - mid) / mid
        return Paint("door", round(k * 20) / 20)
    iy = math.floor(r[1])
    if iy == math.floor(d[1] - 1e-6):
        return Paint("door", 0.12)
    return Paint("door", -0.12 if iy else -0.3)


def accommodation_painter(win_rows=((33, 47),), pitch=24.0, win=14.0):
    """The deckhouse aft: walls in the body colour, a row of windows, doors forward."""
    def p(c):
        if c.bottom:
            return UNDER
        if c.top:
            return Paint("body", 0.05)
        iy = math.floor(c.y)
        top = math.floor(c.box.to[1] - 1e-6)
        if iy == top:
            return Paint("body", 0.15)
        if iy <= math.floor(c.box.frm[1]) + 1:
            return Paint("body", -0.25)
        r, d = rel(c), dims(c)
        a = r[2] if c.side else r[0]
        span = d[2] if c.side else d[0]
        # windows, centred in their bays, symmetric about the face's middle
        off = (span - pitch * math.floor(span / pitch)) / 2
        ia = (a - off) % pitch
        bay = math.floor((a - off) / pitch)
        nb = math.floor(span / pitch)
        if c.front:
            ring = life_ring(abs(c.x) - 17, c.y - (c.box.frm[1] + 21))
            if ring:
                return ring
        for y0, y1 in win_rows:
            if c.front and abs(c.x) < 24:
                break
            if y0 <= c.y < y1 and 0 <= bay < nb:
                w0 = (pitch - win) / 2
                if w0 <= ia < w0 + win:
                    if math.floor(c.y) == y0 or math.floor(c.y) == y1 - 1 or math.floor(ia) in (math.floor(w0), math.floor(w0 + win - 1)):
                        return FRAME
                    return glass(c.y, a, c.y, top=y1, bottom=y0)
                if math.floor(c.y) == y1 and w0 <= ia < w0 + win:
                    return Paint("body", -0.2)
        if c.front and abs(c.x) < 10 and iy < c.box.frm[1] + 32:
            if abs(c.x) > 8 or iy == math.floor(c.box.frm[1]) + 31:
                return FRAME
            return Paint("body", -0.12) if abs(c.x) > 1 else FRAME
        if c.side and 3 <= r[2] < 14 and iy < c.box.frm[1] + 32:
            if r[2] < 4 or r[2] >= 13 or iy == math.floor(c.box.frm[1]) + 31:
                return FRAME
            return Paint("body", -0.1) if math.floor(r[2]) != 11 or iy != math.floor(c.box.frm[1]) + 15 else CHROME_DARK
        return Paint("body", 0.0)
    return p


def life_ring(dx, dy):
    """A lifebuoy eight texels across, orange with four white bands, centred at (0, 0);
    None off it."""
    fx, fy = math.floor(dx) + 0.5, math.floor(dy) + 0.5
    r = math.hypot(fx, fy)
    if 1.9 < r < 4.1:
        return WHITE if min(abs(fx), abs(fy)) < 1 else ORANGE
    return None


def paint_roof(c):
    if c.top:
        r, d = rel(c), dims(c)
        if r[0] < 1.5 or r[0] > d[0] - 1.5 or r[2] < 1.5 or r[2] > d[2] - 1.5:
            return Paint("roof", 0.12)
        return Paint("roof", -0.05)
    if c.bottom:
        return Paint("roof", -0.45)
    iy = math.floor(c.y)
    return Paint("roof", 0.1 if iy == math.floor(c.box.to[1] - 1e-6) else -0.1)


def wheelhouse_painter(g0, g1):
    """A wheelhouse: body below, a glass band g0..g1 all round with slim mullions."""
    def p(c):
        if c.bottom:
            return Paint("body", -0.4)
        if c.top:
            return Paint("body", 0.05)
        iy = math.floor(c.y)
        r, d = rel(c), dims(c)
        a = r[2] if c.side else r[0]
        span = d[2] if c.side else d[0]
        if g0 <= c.y < g1:
            if iy == math.floor(g0) or iy == math.floor(g1 - 1e-6):
                return FRAME
            if a < 1.5 or a > span - 1.5:
                return FRAME
            n = max(1, round(span / 18))
            pitch = span / n
            if (a % pitch) < 1.0 and a > 2:
                return FRAME
            return glass(c.y, a, c.y, top=g1, bottom=g0)
        if iy == math.floor(g0) - 1:
            return Paint("body", -0.18)
        if iy == math.floor(c.box.frm[1]):
            return Paint("body", -0.3)
        return Paint("body", 0.0)
    return p


def add_wheelhouse(s, z0, z1, hw, y0, height=28.0, low=False, mast=True, lift=None):
    """The barge's wheelhouse, on its lifting column unless let down."""
    s.painters.setdefault("wheelhouse", wheelhouse_painter(y0 + 9, y0 + height - 2))
    s.painters.setdefault("roof", paint_roof)
    s.painters.setdefault("steel", paint_steel)
    s.box("wheelhouse", (-hw, y0, z0), (hw, y0 + height, z1), "wheelhouse")
    s.box("wheelhouse_roof", (-hw - 2, y0 + height, z0 - 3), (hw + 2, y0 + height + 3.5, z1 + 1.5), "roof")
    # side lights on the roof's corners, the usual place on a barge
    add_light(s, hw + 0.5, y0 + height - 3, z0 + 3, "side")
    add_light(s, -hw - 0.5, y0 + height - 3, z0 + 3, "side")
    if lift is not None:
        s.box("lift_column", (-5, lift, (z0 + z1) / 2 - 5), (5, y0, (z0 + z1) / 2 + 5), "steel")
        s.box("lift_arm", (-14, lift, z1 - 10), (-8, y0, z1 - 4), "steel")
        s.box("lift_arm", (8, lift, z1 - 10), (14, y0, z1 - 4), "steel")
    if mast:
        s.painters.setdefault("radar", paint_radar)
        top = y0 + height + 3.5
        s.box("radar_post", (-1.5, top, (z0 + z1) / 2 - 1.5), (1.5, top + 4, (z0 + z1) / 2 + 1.5), "steel")
        s.box("radar", (-18, top + 4, (z0 + z1) / 2 - 2), (18, top + 6, (z0 + z1) / 2 + 2), "radar")


def paint_funnel(c):
    """A funnel in the roof colour with a black top."""
    iy = math.floor(c.y)
    top = math.floor(c.box.to[1] - 1e-6)
    if c.top:
        r, d = rel(c), dims(c)
        if 2 <= r[0] < d[0] - 2 and 2 <= r[2] < d[2] - 2:
            return hexc("#0d0d0f")
        return hexc("#2a2b2f")
    if c.bottom:
        return Paint("roof", -0.5)
    if iy > top - 4:
        return hexc("#1e1f22") if iy != top else hexc("#2e3034")
    if iy == top - 4:
        return Paint("roof", -0.35)
    if iy == top - 9:
        return Paint("stripe", 0.0)
    return Paint("roof", 0.08 if c.side else -0.05)


def paint_radar(c):
    if c.top:
        return WHITE
    return WHITE_SH if c.side else mix(WHITE, (0, 0, 0), 0.15)


def barge(kind, low=False):
    s = Ship()
    half = BARGE_L / 2
    s.painters.update({"dark": paint_dark, "steel": paint_steel, "lamp": lamp_painter, "roof": paint_roof,
                       "mast": paint_mast, "white_rail": paint_white_rail})
    if kind == "tank":
        deck_paint = tank_deck
    else:
        deck_paint = gangboard
    barge_hull(s, deck_paint)
    if kind == "dry":
        cargo_hatches(s)
    elif kind == "container":
        container_stack(s, low)
    else:
        tank_deck_gear(s)
    # Accommodation aft, a car on its roof by the wheelhouse.
    s.painters["acc"] = accommodation_painter()
    acc_w = BARGE_W - 10
    s.box("accommodation", (-acc_w, BARGE_DECK, ACC_FROM), (acc_w, 64, ACC_TO), "acc")
    s.box("acc_roof", (-acc_w - 1.5, 64, ACC_FROM - 1.5), (acc_w + 1.5, 67, ACC_TO + 1.5), "roof")
    roof_y = 67.0
    wz0, wz1 = m(41.9), m(45.5)
    if low:
        add_wheelhouse(s, wz0, wz1, m(2.4), roof_y, height=28.0, mast=False)
    else:
        add_wheelhouse(s, wz0, wz1, m(2.4), m(5.6), height=28.0, lift=roof_y)
    add_car(s, -(acc_w - 18), roof_y, m(44.2), 3 if kind != "tank" else 0, length=68)
    # The engine's exhaust, aft on the roof.
    s.painters["funnel"] = paint_funnel
    s.box("exhaust", (50, roof_y, ACC_TO - 26), (62, 95, ACC_TO - 12), "funnel")
    # Railing round the roof's after part, below the rider's eyes.
    railing(s, (acc_w - 1, m(46.5)), (acc_w - 1, ACC_TO - 1), roof_y, 3)
    railing(s, (-acc_w + 1, m(46.5)), (-acc_w + 1, ACC_TO - 1), roof_y, 3)
    railing(s, (-acc_w + 1, ACC_TO - 1), (acc_w - 1, ACC_TO - 1), roof_y, 4, ends=False, trim=2.0)
    # Stern light and flagstaff.
    add_light(s, 0, 28, half - 6, "white")
    s.box("flagstaff", (-0.75, 28, half - 12), (0.75, 60, half - 10.5), "mast")
    # Bow mast with the masthead light, an anchor winch.
    s.box("bow_mast", (-1.5, BARGE_DECK + 4, -half + 54), (1.5, 52, -half + 57), "mast")
    add_light(s, 0, 52, -half + 55.5, "white")
    s.box("winch", (-16, BARGE_DECK + 4, -half + 30), (16, BARGE_DECK + 12, -half + 40), "steel")
    s.box("winch_drum", (-22, BARGE_DECK + 5, -half + 31.5), (22, BARGE_DECK + 11, -half + 38.5), "dark")
    ids = {"dry": "ship_barge", "container": "ship_container_barge", "tank": "ship_tank_barge"}
    mid = ids[kind] + ("_low" if low else "")
    livery = {"body": "minecraft:white_concrete", "lower": "minecraft:black_concrete",
              "stripe": "minecraft:red_concrete", "door": "minecraft:gray_concrete",
              "roof": "minecraft:gray_concrete"}
    # Rust seat: 49.5 m aft of the middle, eyes 5.8 m up, on the roof by the wheelhouse.
    return Model(mid, s.boxes, s.painters, livery, eye=(0.0, m(5.8), m(49.5)))


def cargo_hatches(s):
    s.painters["coaming"] = paint_coaming
    s.painters["lid"] = paint_lid
    n = 16
    seg = (HOLD_TO - HOLD_FROM) / n
    cw = BARGE_W - 12
    for k in range(n):
        z = HOLD_FROM + k * seg
        s.box("coaming", (-cw, BARGE_DECK, z), (cw, 37, z + seg), "coaming")
        s.box("lid", (-cw - 2, 37, z + 1), (cw + 2, 42, z + seg - 1), "lid")


# ---------------------------------------------------------------- containers

BOX_COLOURS = [hexc("#2e5c9a"), hexc("#a8352c"), hexc("#3f8a52"), hexc("#d4772a"), hexc("#8b9097"),
               hexc("#d9dbdc"), hexc("#26355e"), hexc("#7b4a33"), hexc("#2a8a8e"), hexc("#c9a43a")]


def container_painter(col):
    hi = mix(col, (255, 255, 255), 0.18)
    lo = mix(col, (0, 0, 0), 0.22)
    lo2 = mix(col, (0, 0, 0), 0.4)

    def p(c):
        r, d = rel(c), dims(c)
        if c.bottom:
            return lo2
        if c.top:
            if r[0] < 1 or r[0] > d[0] - 1 or r[2] < 1 or r[2] > d[2] - 1:
                return lo
            return hi if math.floor(r[2]) % 6 else col
        iy = math.floor(r[1])
        top = math.floor(d[1] - 1e-6)
        a = r[2] if c.side else r[0]
        span = d[2] if c.side else d[0]
        if iy == top:
            return hi
        if iy == 0:
            return lo2
        if a < 1.5 or a > span - 1.5:
            return lo
        if c.side:
            k = math.floor(a) % 4
            return mix(col, (255, 255, 255), 0.07) if k == 0 else (lo if k == 2 else col)
        # ends: doors at the back, corrugated at the front
        if c.back:
            ia = math.floor(a)
            mid = span / 2
            if abs(a - mid) < 0.75:
                return lo2
            if ia in (math.floor(mid / 2) - 2, math.floor(mid / 2) + 2, math.floor(mid * 1.5) - 2, math.floor(mid * 1.5) + 2):
                return hi
            return col
        return lo if math.floor(r[1]) % 4 == 1 else col
    return p


def container_stack(s, low):
    s.painters["coaming_grey"] = lambda c: (hexc("#7d8389") if c.top else (UNDER if c.bottom else (hexc("#8f959b") if math.floor(c.y) == 28 else hexc("#6f757b"))))
    cw = BARGE_W - 10
    n = 8
    seg = (HOLD_TO - HOLD_FROM) / n
    for k in range(n):
        z = HOLD_FROM + k * seg
        s.box("coaming", (-cw, BARGE_DECK, z), (cw, 29, z + seg), "coaming_grey")
    for i, col in enumerate(BOX_COLOURS):
        s.painters[f"box{i}"] = container_painter(col)
    width = 39.0
    xs = [-2 * width, -width, 0.0, width]
    length = 195.0
    gap = 3.0
    bays = 6
    total = bays * (length + gap) + 97 + gap
    z = HOLD_FROM + (HOLD_TO - HOLD_FROM - total) / 2
    tiers = 1 if low else 2
    pattern = [6, 0, 1, 4, 6, 2, 5, 0, 3, 6, 7, 1, 0, 8, 4, 6, 9, 1, 0, 2, 6, 5, 1, 3, 0, 6, 4, 7]
    k = 0
    for bay in range(bays + 1):
        ln = length if bay < bays else 97.0
        for tier in range(tiers):
            for col in range(4):
                if tier == 1 and (bay, col) in ((1, 0), (3, 3), (4, 0), (4, 1), (6, 2), (6, 3), (0, 3)):
                    continue
                y0 = 29 + tier * 41
                ci = pattern[(bay * 4 + col + tier * 7) % len(pattern)]
                s.box("container", (xs[col], y0, z), (xs[col] + width, y0 + 41, z + ln), f"box{ci}")
                k += 1
        z += ln + gap


# ---------------------------------------------------------------- tank deck


def tank_deck(c):
    r = rel(c)
    ax = abs(c.x)
    if ax > BARGE_W:
        return Paint("stripe", 0.25)
    if ax > BARGE_W - 1:
        return TANK_DECK_HI
    if math.floor(r[2]) % 48 == 0:
        return TANK_DECK_LO
    return TANK_DECK


def paint_pipe(c):
    if c.top:
        return PIPE_HI
    if c.bottom:
        return PIPE_LO
    iy = math.floor(c.y - c.box.frm[1])
    return PIPE if iy else PIPE_LO


def paint_walkway(c):
    if c.top:
        r = rel(c)
        return DECK_HI if math.floor(r[2]) % 3 == 0 else DECK
    if c.bottom:
        return DECK_LO
    return YELLOW_LO


def paint_handrail(c):
    return YELLOW if (c.top or c.side) else YELLOW_LO


def paint_dome(c):
    """A tank hatch: a grey coaming, its round lid with a handwheel."""
    r, d = rel(c), dims(c)
    if c.top:
        cx, cz = d[0] / 2, d[2] / 2
        rr = math.hypot(math.floor(r[0]) + 0.5 - cx, math.floor(r[2]) + 0.5 - cz)
        if rr < 2:
            return VALVE
        if rr < 6.5:
            return PIPE_HI if rr < 5.5 else PIPE_LO
        return PIPE
    iy = math.floor(r[1])
    return PIPE_LO if iy == 0 else (PIPE_HI if iy == math.floor(d[1] - 1e-6) else PIPE)


def tank_deck_gear(s):
    s.painters.update({"pipe": paint_pipe, "walkway": paint_walkway, "handrail": paint_handrail,
                       "dome": paint_dome, "valve": paint_valve, "crane": paint_crane, "vent": paint_vent})
    z0, z1 = HOLD_FROM + 8, HOLD_TO - 8
    n = 8
    seg = (z1 - z0) / n
    d = BARGE_DECK
    for k in range(n):
        za, zb = z0 + k * seg, z0 + (k + 1) * seg
        zm = (za + zb) / 2
        # the catwalk along the middle, on a leg a tank, yellow handrails both sides
        s.box("catwalk", (-7, 38, za), (7, 40.5, zb), "walkway")
        s.box("catwalk_leg", (-2, d, zm - 2), (2, 38, zm + 2), "steel")
        s.x2("handrail", (6, 54, za), (7, 55.5, zb), "handrail")
        s.x2("handrail_mid", (6, 47, za), (7, 48, zb), "handrail")
        s.x2("handrail_post", (5.6, 40.5, za + 0.5), (7.4, 54, za + 2.3), "handrail")
        # the tank's two hatches, a vent valve, a branch from the pipes with its valve
        s.x2("dome", (27, d, zm - 9), (43, d + 6, zm + 9), "dome")
        s.box("vent", (21, d, zm + 22), (24, d + 22, zm + 25), "vent")
        s.box("vent_head", (19.5, d + 22, zm + 20.5), (25.5, d + 26, zm + 26.5), "vent")
        s.box("branch", (-27, d + 1.5, zm - 1.5), (-15, d + 4.5, zm + 1.5), "pipe")
        s.box("branch_valve", (-24, d + 4.5, zm - 3), (-19, d + 6.5, zm + 3), "valve")
    # pipes along the deck, in runs
    for k in range(4):
        za, zb = z0 + k * (z1 - z0) / 4, z0 + (k + 1) * (z1 - z0) / 4
        s.box("pipe", (-15, d + 1, za), (-10, d + 6, zb), "pipe")
        s.box("pipe", (10, d + 1.5, za), (14, d + 5.5, zb), "pipe")
        s.box("pipe", (16, d + 1.5, za), (19, d + 4.5, zb), "pipe")
    # the manifold amidships: two lines across to both sides, valves and a drip tray at
    # each end, and the hose crane
    zc = z0 + 4 * seg
    for dz in (-7.0, 7.0):
        s.box("manifold", (-BARGE_W + 8, d + 7, zc + dz - 2.5), (BARGE_W - 8, d + 12, zc + dz + 2.5), "pipe")
        s.box("manifold_riser", (-15, d + 6, zc + dz - 2), (-10, d + 7, zc + dz + 2), "pipe")
    for x in (-1, 1):
        s.box("drip_tray", (x * (BARGE_W - 6) - 8, d, zc - 14), (x * (BARGE_W - 6) + 8, d + 3, zc + 14), "steel")
    for dz in (-7.0, 7.0):
        s.x2("manifold_valve", (BARGE_W - 22, d + 6, zc + dz - 4), (BARGE_W - 16, d + 13, zc + dz + 4), "valve")
    s.box("crane_post", (26, d, zc - 3), (32, d + 52, zc + 3), "crane")
    s.box("crane_jib", (-36, d + 46, zc - 2), (26, d + 50, zc + 2), "crane")
    s.box("crane_hook", (-35, d + 32, zc - 1), (-33, d + 46, zc + 1), "dark")


def paint_valve(c):
    if c.top:
        return mix(VALVE, (255, 255, 255), 0.2)
    if c.bottom:
        return mix(VALVE, (0, 0, 0), 0.4)
    return VALVE if c.side else mix(VALVE, (0, 0, 0), 0.15)


def paint_crane(c):
    if c.bottom:
        return YELLOW_LO
    if c.top:
        return mix(YELLOW, (255, 255, 255), 0.15)
    iy = math.floor(c.y)
    if c.box.name.startswith("crane_post") and iy % 10 == 0:
        return YELLOW_LO
    return YELLOW if c.side else mix(YELLOW, (0, 0, 0), 0.1)


def paint_vent(c):
    if c.box.name.startswith("vent_head"):
        if c.top:
            return hexc("#3f8a52")
        return hexc("#2f6b3f") if math.floor(c.y) % 2 else hexc("#3f8a52")
    return paint_pipe(c)


# ---------------------------------------------------------------- river cruise ship

CR_L, CR_HW = m(135.0), m(11.4) / 2
CR_RAIL = 2.2
CR_W = CR_HW - CR_RAIL
CR_DOWN, CR_DECK = -26.0, 16.0
CR_SW = 87.0                              # the superstructure's half-width
CR_FROM, CR_TO = -m(55.5), m(64.5)        # Rust: half - 12 .. -half + 3, ends swapped
CR_TOP = 122.0                            # roof of the upper deck (7.6 m)
CR_SUN = 124.0                            # the sun deck (7.75 m)
DECKS = (16.0, 51.0, 87.0, CR_TOP)        # main, middle and upper deck floors, the roof
MODULE = 72.0                             # a cabin, 4.5 m
GLASS_PALE = hexc("#a9bccb")
GLASS_PALE_HI = hexc("#d3e0ea")


def cabin_side(c, a, y):
    """A cabin module's side, a from the module's start: a window on the main deck,
    French balconies above, white pillars between."""
    iy = math.floor(y)
    ia = a - MODULE / 2
    for k, (f0, f1) in enumerate(zip(DECKS, DECKS[1:])):
        if f0 <= y < f1:
            break
    if iy == math.floor(f1) - 1:
        return Paint("body", -0.3)            # the deck's edge, shadowed
    if iy == math.floor(f0):
        return Paint("body", 0.12)
    if math.floor(a) == 0 or math.floor(a) == MODULE - 1:
        return Paint("body", -0.22)           # panel seam between cabins
    if k == 0:
        if abs(ia) < 15 and 30 <= y < 44:
            if abs(ia) >= 14 or iy in (30, 43):
                return FRAME
            return glass(y, a, iy, top=44, bottom=30)
        return Paint("body", 0.0 if iy > 20 else -0.08)
    g0, g1 = f0 + 3, f1 - 3
    if abs(ia) < 26 and g0 <= y < g1:
        if abs(ia) >= 25 or iy in (math.floor(g0), math.floor(g1) - 1):
            return FRAME
        if iy == math.floor(g0) + 14:
            return WHITE                        # the French balcony's rail
        if iy == math.floor(g0) + 13:
            return hexc("#1a2433")
        if abs(ia) < 0.6:
            return FRAME                        # the door's meeting stiles
        return glass(y, a, iy, top=g1, bottom=g0)
    return Paint("body", 0.04 if iy == math.floor(f0) + 1 else 0.0)


def lounge_side(c, a, y, span):
    """Panoramic glass of a lounge: tall panes with slim mullions on the upper and middle
    decks, cabins' windows on the main deck."""
    iy = math.floor(y)
    for k, (f0, f1) in enumerate(zip(DECKS, DECKS[1:])):
        if f0 <= y < f1:
            break
    if iy == math.floor(f1) - 1:
        return Paint("body", -0.3)
    if iy == math.floor(f0):
        return Paint("body", 0.12)
    if a < 4 or a > span - 4:
        return Paint("body", -0.05)
    if k == 0:
        ia = (a - 4) % 36 - 18
        if abs(ia) < 11 and 30 <= y < 44 and a > 8 and a < span - 8:
            if abs(ia) >= 10 or iy in (30, 43):
                return FRAME
            return glass(y, a, iy, top=44, bottom=30)
        return Paint("body", 0.0)
    g0, g1 = f0 + 3, f1 - 2
    if g0 <= y < g1:
        if iy in (math.floor(g0), math.floor(g1) - 1) or (a - 4) % 16 < 1:
            return FRAME
        return glass(y, a, iy, top=g1, bottom=g0)
    return Paint("body", 0.0)


def cruise_painter(kind):
    def p(c):
        if c.bottom:
            return UNDER
        if c.top:
            return Paint("body", 0.05)
        r, d = rel(c), dims(c)
        if c.face in ("east", "west"):
            a = r[2] if c.face == "west" else d[2] - r[2]
            span = d[2]
        else:
            a = r[0] if c.face == "south" else d[0] - r[0]
            span = d[0]
        if kind == "cabins" and c.face in ("east", "west"):
            return cabin_side(c, a % MODULE, c.y)
        return lounge_side(c, a, c.y, span)
    return p


def paint_sundeck(c):
    """The sun deck: the deck colour, a running track, a lit edge."""
    if c.bottom:
        return Paint("body", -0.35)
    if not c.top:
        iy = math.floor(c.y)
        return Paint("stripe", 0.12) if iy == math.floor(c.box.to[1] - 1e-6) else Paint("stripe", -0.08)
    ax = abs(c.x)
    if ax > CR_SW - 1:
        return Paint("door", 0.25)
    if 64 <= ax < 74:
        return hexc("#3f7f8c") if math.floor(ax) not in (64, 73) else hexc("#e6ecef")
    r = rel(c)
    return Paint("door", -0.06 if math.floor(r[2]) % 24 == 0 else 0.04)


def paint_glass_rail(c):
    """A glass balustrade: steel posts, a white handrail, pale glass."""
    iy = math.floor(c.y - c.box.frm[1])
    h = math.floor(c.box.to[1] - c.box.frm[1] - 1e-6)
    if c.top:
        return WHITE
    if c.bottom:
        return WHITE_SH
    if iy >= h - 1:
        return WHITE
    if c.face in ("east", "west"):
        a = rel(c)[2]
    else:
        a = rel(c)[0]
    if math.floor(a) % 24 in (0, 23) or iy == 0:
        return WHITE_SH
    ia, ib = math.floor(a), iy
    if (ia + ib) % 17 == 0:
        return GLASS_PALE_HI
    return GLASS_PALE


def paint_lounger(c):
    if c.top:
        lz = rel(c)[2]
        return WHITE if lz > 22 else (hexc("#f2f2f0") if math.floor(lz) % 3 else hexc("#d9dcde"))
    if c.bottom:
        return hexc("#555a60")
    return hexc("#8e949b")


def paint_planter(c):
    if c.top:
        r = rel(c)
        return hexc("#4f8a3a") if (math.floor(r[0]) + math.floor(r[2])) % 3 else hexc("#3b6e2c")
    return TEAK if math.floor(c.y) % 4 else TEAK_LO


def paint_awning(c):
    """The sun deck's awning, in the roof colour, scalloped edge."""
    if c.bottom:
        return Paint("roof", -0.3)
    if c.top:
        r = rel(c)
        return Paint("roof", 0.08 if math.floor(r[0]) % 12 < 6 else -0.02)
    r = rel(c)
    a = r[2] if c.side else r[0]
    iy = math.floor(c.y - c.box.frm[1])
    if iy == 0 and math.floor(a) % 6 >= 3:
        return Paint("roof", -0.25)
    return Paint("roof", 0.0)


def add_cruise_wheelhouse(s, low):
    """The wheelhouse forward on the sun deck, glazed all round, its wings to the sides;
    on its lifting platform, or let down onto the deck."""
    z0, z1 = -m(53.5), -m(48.5)
    y0 = CR_SUN if low else m(8.6)
    h = 26.5
    hw = 74.0
    s.painters["cr_wheelhouse"] = wheelhouse_painter(y0 + 7, y0 + h - 2)
    s.box("wheelhouse", (-hw, y0, z0), (hw, y0 + h, z1), "cr_wheelhouse")
    roof_top = (m(9.55) if low else m(10.4))
    s.box("wheelhouse_roof", (-hw - 1.5, y0 + h, z0 - 3), (hw + 1.5, roof_top, z1 + 1.5), "roof")
    add_light(s, hw + 1.5, y0 + h - 4, z0 + 4, "side")
    add_light(s, -hw - 1.5, y0 + h - 4, z0 + 4, "side")
    if not low:
        s.box("wheelhouse_base", (-60, CR_SUN, z0 + 6), (60, y0, z1 - 6), "lift_base")
        # the masthead light and radar, folding, on the roof
        s.box("mast", (-1, roof_top, (z0 + z1) / 2 - 1), (1, roof_top + 6, (z0 + z1) / 2 + 1), "steel")
        add_light(s, 0, roof_top + 6, (z0 + z1) / 2, "white", size=1.5)
        s.painters.setdefault("radar", paint_radar)
        s.box("radar", (-16, roof_top, z1 - 12), (16, roof_top + 2, z1 - 8), "radar")
    else:
        # mast laid down along the roof
        s.box("mast_down", (-1, roof_top - 1.5 - 2, z1 - 2), (1, roof_top - 1.5 + 0, z1 + 6), "steel")


def paint_lift_base(c):
    """The lifting wheelhouse's platform: rams and their housings under it."""
    if c.top or c.bottom:
        return STEEL
    r = rel(c)
    a = r[0] if (c.front or c.back) else r[2]
    if math.floor(a) % 20 in (4, 5, 14, 15):
        return CHROME_DARK
    return mix(STEEL, (0, 0, 0), 0.25)


def river_cruise(low=False):
    s = Ship()
    half = CR_L / 2
    s.painters.update({"dark": paint_dark, "steel": paint_steel, "lamp": lamp_painter, "roof": paint_roof,
                       "mast": paint_mast, "white_rail": paint_white_rail, "radar": paint_radar,
                       "cabins": cruise_painter("cabins"), "lounge": cruise_painter("lounge"),
                       "sundeck": paint_sundeck, "glass_rail": paint_glass_rail, "lounger": paint_lounger,
                       "planter": paint_planter, "awning": paint_awning, "bollard": paint_bollard,
                       "lift_base": paint_lift_base})
    style = HullStyle(
        deck=lambda c: Paint("stripe", 0.25) if abs(c.x) > CR_W else (DECK if math.floor(rel(c)[2]) % 24 else DECK_LO),
        inner=lambda c, cap=False: Paint("body", 0.1) if cap else Paint("body", -0.12),
        band_y0=9.0, marks=((-half + 46, -half + 50), (half - 30, half - 26)))
    bow = dict(pts=[(220.0, CR_W), (170, CR_W - 3), (120, CR_W - 15), (72, CR_W - 37), (34, CR_W - 63), (14, CR_W - 80)],
               tops=[24, 25, 26, 27, 28], bots=[-26, -26, -24, -20, -14], t=8.0, fill_top=CR_DECK + 2,
               tip=(0.0, CR_W - 80, 30, -10), anchor=3)
    stern = dict(pts=[(160.0, CR_W), (96, CR_W - 0.5), (44, CR_W - 5), (16, CR_W - 18)],
                 tops=[20, 21, 22], bots=[-26, -20, -12], t=8.0, fill_top=CR_DECK + 1,
                 tip=(0.0, CR_W - 18, 23, -6))
    build_hull(s, style, CR_L, CR_HW, CR_RAIL, CR_DOWN, CR_DECK, bow, stern, 10)
    # The superstructure: a forward lounge with its corners cut, seven segments of three
    # cabins, the aft lounge and restaurant.
    ch = 27.0
    fz = CR_FROM
    s.box("lounge_front", (-CR_SW + ch, CR_DECK, fz), (CR_SW - ch, CR_TOP, fz + ch), "lounge")
    s.box("lounge_fwd", (-CR_SW, CR_DECK, fz + ch), (CR_SW, CR_TOP, fz + 168), "lounge")
    corner = Box("lounge_corner", (CR_SW - 20, CR_DECK, fz + ch - ch * math.sqrt(2) - 0.3),
                 (CR_SW, CR_TOP, fz + ch + 0.3), "lounge", origin=(CR_SW, 0, fz + ch), rot=(0, 45, 0))
    s.take(corner)
    s.boxes.append(corner.mirrored(s._name("lounge_corner_l")))
    z = fz + 168
    for k in range(7):
        s.box("cabins", (-CR_SW, CR_DECK, z), (CR_SW, CR_TOP, z + 3 * MODULE), "cabins")
        z += 3 * MODULE
    az = z
    s.box("lounge_aft", (-CR_SW, CR_DECK, az), (CR_SW, CR_TOP, CR_TO - ch), "lounge")
    s.box("lounge_back", (-CR_SW + ch, CR_DECK, CR_TO - ch), (CR_SW - ch, CR_TOP, CR_TO), "lounge")
    corner = Box("lounge_corner_aft", (CR_SW - 20, CR_DECK, CR_TO - ch - 0.3),
                 (CR_SW, CR_TOP, CR_TO - ch + ch * math.sqrt(2) + 0.3), "lounge",
                 origin=(CR_SW, 0, CR_TO - ch), rot=(0, -45, 0))
    s.take(corner)
    s.boxes.append(corner.mirrored(s._name("lounge_corner_aft_l")))
    # The sun deck over it all, in pieces like the decks below.
    cuts = [CR_FROM, fz + 168] + [fz + 168 + 3 * MODULE * (k + 1) for k in range(7)] + [CR_TO]
    for za, zb in zip(cuts, cuts[1:]):
        s.box("sundeck", (-CR_SW, CR_TOP, za), (CR_SW, CR_SUN, zb), "sundeck")
    if not low:
        for za, zb in zip(cuts, cuts[1:]):
            s.x2("glass_rail", (CR_SW - 1.5, CR_SUN, za + 1), (CR_SW, CR_SUN + 14, zb - 1), "glass_rail")
        s.box("glass_rail_fwd", (-CR_SW + 1.5, CR_SUN, CR_FROM + 0.5), (CR_SW - 1.5, CR_SUN + 14, CR_FROM + 2), "glass_rail")
        s.box("glass_rail_aft", (-CR_SW + 1.5, CR_SUN, CR_TO - 2), (CR_SW - 1.5, CR_SUN + 14, CR_TO - 0.5), "glass_rail")
    else:
        # railings folded flat along the deck's edges
        for za, zb in zip(cuts, cuts[1:]):
            s.x2("rail_folded", (CR_SW - 14, CR_SUN, za + 1), (CR_SW - 1, CR_SUN + 1.5, zb - 1), "glass_rail")
    add_cruise_wheelhouse(s, low)
    # The bar and its awning amidships aft (the Rust model's roof-coloured box), planters,
    # loungers in rows.
    s.box("bar", (-32, CR_SUN, m(30.5)), (32, CR_SUN + 22, m(34.5)), "lounge")
    s.box("bar_roof", (-34, CR_SUN + 22, m(30.5) - 2), (34, CR_SUN + 24, m(34.5) + 2), "roof")
    if not low:
        s.box("awning", (-60, 156, m(28.0)), (60, 158, m(37.0)), "awning")
        for x in (-58, 56):
            for zz in (m(28.0) + 1, m(37.0) - 3):
                s.box("awning_post", (x, CR_SUN, zz), (x + 2, 156, zz + 2), "white_rail")
    for zz in (-m(44.0), -m(41.0)):
        s.x2("planter", (20, CR_SUN, zz), (52, CR_SUN + 6, zz + 24), "planter")
    for zc in (m(8.0), m(12.0), m(51.0), m(55.0), m(59.0)):
        for x in (-46, -30, 30, 46):
            s.box("lounger", (x - 6, CR_SUN, zc - 16), (x + 6, CR_SUN + 5, zc + 16), "lounger")
    # Liferafts in their racks along the rails.
    s.painters["liferaft"] = paint_liferaft
    for zc in (-m(30.0), m(2.0), m(40.0)):
        s.x2("liferaft", (CR_SW - 22, CR_SUN, zc), (CR_SW - 6, CR_SUN + 10, zc + 30), "liferaft")
    # Bow: flagstaff, mooring winch; stern light.
    s.box("winch", (-14, CR_DECK + 2, -half + 80), (14, CR_DECK + 10, -half + 92), "steel")
    s.box("jackstaff", (-0.75, 28, -half + 18), (0.75, 70, -half + 19.5), "mast")
    add_light(s, 0, 70, -half + 18.75, "white", size=1.5)
    add_light(s, 0, CR_TOP - 6, CR_TO + 0.5, "white")
    mid = "ship_river_cruise" + ("_low" if low else "")
    livery = {"body": "minecraft:white_concrete", "lower": "minecraft:blue_concrete",
              "stripe": "minecraft:cyan_concrete", "door": "minecraft:light_gray_concrete",
              "roof": "minecraft:blue_concrete"}
    # Rust seats at -45, -20, 5, 30 m, eyes 9.35 m up on the sun deck: ours 5 m forward.
    return Model(mid, s.boxes, s.painters, livery, eye=(0.0, m(9.35), -m(5.0)))

# ---------------------------------------------------------------- car ferry

CF_L, CF_HW = m(72.0), m(16.0) / 2
CF_RAIL = 2.5
CF_W = CF_HW - CF_RAIL
CF_DOWN, CF_DECK = -40.0, 32.0              # 2.5 m under, the car deck 2.0 m up
CF_CASE = 92.0                              # the casings' inner faces
CF_CASE_Z = 340.0
CF_UPPER = 136.0                            # casings' roof, 8.5 m
LANES = (-66.0, -22.0, 22.0, 66.0)
HAZARD_Y = hexc("#e3b22a")


def car_deck(c):
    """The car deck in the deck colour: lanes marked in white, the ramps' hinges."""
    x = c.x
    ax = abs(x)
    if ax > CF_W:
        return Paint("stripe", 0.25)
    name = c.box.name
    lz = rel(c)[2] if name.startswith("sheer") else abs(c.z)
    if "_tip" in name:
        # the ramp: ribbed, a hazard-striped edge at the end
        d = CF_L / 2 - abs(c.z)
        if d < CF_RAIL + 3:
            return HAZARD_Y if (math.floor(x) + math.floor(d)) % 8 < 4 else hexc("#1d1e21")
        return Paint("door", 0.1 if math.floor(d) % 4 == 0 else -0.05)
    ix = math.floor(x)
    if ix in (-1, 0) or ix in (43, 44, -45, -44):
        if math.floor(lz) % 32 < 18:
            return hexc("#d9dbd6")
    if math.floor(ax) == 88:
        return HAZARD_Y
    return Paint("door", -0.04 if math.floor(lz) % 16 == 0 else 0.0)


def casing_painter():
    """The passenger casings either side: two decks of windows outboard, doors inboard."""
    def p(c):
        if c.bottom:
            return UNDER
        if c.top:
            return Paint("door", 0.05) if abs(c.x) < CF_W - 1 else Paint("body", 0.15)
        iy = math.floor(c.y)
        r, d = rel(c), dims(c)
        if iy == math.floor(c.box.to[1] - 1e-6):
            return Paint("body", 0.15)
        if iy in (83, 84):
            return Paint("body", -0.28 if iy == 83 else 0.1)
        if iy <= CF_DECK + 1:
            return Paint("body", -0.3)
        if (c.face in ("east", "west") and outward(c)) or c.face in ("north", "south"):
            if 126 <= iy <= 131:
                return Paint("stripe", 0.0 if iy > 126 else -0.15)
        if c.face in ("east", "west"):
            a = r[2]
            ia = a % 34
            out = outward(c)
            if out:
                for y0, y1 in ((52, 72), (98, 120)):
                    if y0 <= c.y < y1 and 4 <= ia < 30:
                        if iy in (y0, y1 - 1) or math.floor(ia) in (4, 29):
                            return FRAME
                        return glass(c.y, a, iy, top=y1, bottom=y0)
            else:
                # inboard: doors to the stairs, small windows above
                ib = a % 85
                if 36 <= ib < 50 and iy < CF_DECK + 34:
                    if math.floor(ib) in (36, 49) or iy == CF_DECK + 33:
                        return FRAME
                    return Paint("body", -0.14)
                if 100 <= c.y < 116 and 6 <= a % 34 < 28:
                    if iy in (100, 115) or math.floor(a % 34) in (6, 27):
                        return FRAME
                    return glass(c.y, a, iy, top=116, bottom=100)
            return Paint("body", 0.0)
        # the casing's ends: a door and a window above, a lifebuoy
        ax = r[0]
        ring = life_ring(ax - (d[0] - 7) if c.face == "north" else (d[0] - ax) - (d[0] - 7), c.y - 72)
        if ring and abs(c.x) > CF_CASE + 20:
            return ring
        if 8 <= ax < d[0] - 8 and iy < CF_DECK + 34:
            if math.floor(ax) in (8, math.floor(d[0]) - 9) or iy == CF_DECK + 33:
                return FRAME
            return Paint("body", -0.14)
        if 6 <= ax < d[0] - 6 and 98 <= c.y < 120:
            if math.floor(ax) in (6, math.floor(d[0]) - 7) or iy in (98, 119):
                return FRAME
            return glass(c.y, ax, iy, top=120, bottom=98)
        return Paint("body", 0.0)
    return p


def gantry_painter(c):
    """The lounge across the car deck: glass fore and aft, its underside shadowed."""
    if c.bottom:
        return Paint("body", -0.35) if math.floor(rel(c)[2]) % 24 else Paint("body", -0.45)
    if c.top:
        return Paint("door", 0.05)
    iy = math.floor(c.y)
    r, d = rel(c), dims(c)
    if iy == math.floor(c.box.to[1] - 1e-6):
        return Paint("body", 0.15)
    if iy == math.floor(c.box.frm[1]):
        return Paint("body", -0.3)
    if c.front or c.back:
        a = r[0]
        if 104 <= c.y < 130 and 4 <= a < d[0] - 4:
            if iy in (104, 129) or math.floor(a - 4) % 23 == 0:
                return FRAME
            return glass(c.y, a, iy, top=130, bottom=104)
    return Paint("body", 0.0)


def wing_painter(g0):
    """A bridge wing: glass on its outer sides from g0 over its floor, the roof's shadow
    above."""
    def p(c):
        if c.bottom:
            return Paint("body", -0.4)
        if c.top:
            return Paint("body", 0.05)
        iy = math.floor(c.y)
        r = rel(c)
        y0 = c.box.frm[1]
        if y0 + g0 <= c.y < c.box.to[1] - 2:
            a = r[2] if c.side else r[0]
            if iy in (math.floor(y0 + g0), math.floor(c.box.to[1] - 3)) or math.floor(a) % 16 == 0:
                return FRAME
            return glass(c.y, a, iy, top=c.box.to[1] - 2, bottom=y0 + g0)
        if iy == math.floor(y0 + g0) - 1:
            return Paint("body", -0.18)
        if iy == math.floor(y0):
            return Paint("body", -0.3)
        return Paint("body", 0.0)
    return p


def paint_liferaft(c):
    """Liferaft canisters on their rack: white drums, dark bands."""
    r = rel(c)
    if c.top:
        return WHITE if math.floor(r[2]) % 10 not in (0, 9) else WHITE_SH
    lz = math.floor(r[2]) % 10
    if c.side and lz in (0, 9):
        return STEEL
    if c.side and lz in (2, 7):
        return hexc("#3a3c42")
    return WHITE_SH if c.side else WHITE


def car_ferry():
    s = Ship()
    half = CF_L / 2
    s.painters.update({"dark": paint_dark, "steel": paint_steel, "lamp": lamp_painter, "roof": paint_roof,
                       "mast": paint_mast, "white_rail": paint_white_rail, "radar": paint_radar,
                       "funnel": paint_funnel, "casing": casing_painter(), "gantry": gantry_painter,
                       "wing": wing_painter(12), "glass_rail": paint_glass_rail, "liferaft": paint_liferaft,
                       "bollard": paint_bollard,
                       "wheelhouse": wheelhouse_painter(CF_UPPER + 12, CF_UPPER + 38)})
    style = HullStyle(
        deck=car_deck,
        inner=lambda c, cap=False: Paint("body", 0.1) if cap else Paint("body", -0.12),
        band_y0=22.0, stripe=6, marks=((-half + 60, -half + 64), (half - 64, half - 60)))
    end = dict(pts=[(230.0, CF_W), (170, CF_W - 3.5), (110, CF_W - 15.5), (60, CF_W - 32.5), (22, CF_W - 49.5)],
               tops=[44, 45, 46, 47], bots=[-40, -38, -32, -24], t=8.0, fill_top=CF_DECK,
               tip=(0.0, CF_W - 49.5, CF_DECK + 3, -16), anchor=2, slab=6.0)
    build_hull(s, style, CF_L, CF_HW, CF_RAIL, CF_DOWN, CF_DECK, end, None, 4, double=True)
    # Casings along both sides, two decks of passenger rooms, in identical segments.
    n = 4
    seg = 2 * CF_CASE_Z / n
    for k in range(n):
        za = -CF_CASE_Z + k * seg
        s.x2("casing", (CF_CASE, CF_DECK, za), (CF_W, CF_UPPER, za + seg), "casing")
    # The lounge across the car deck at the upper deck, its roof the bridge deck.
    for za, zb in ((-120.0, 0.0), (0.0, 120.0)):
        s.box("gantry", (-CF_CASE, 100, za), (CF_CASE, CF_UPPER, zb), "gantry")
    # The wheelhouse in the middle, glazed fore and aft alike, its wings out to both sides.
    y0 = CF_UPPER
    s.box("wheelhouse", (-56, y0, -36), (56, y0 + 40, 36), "wheelhouse")
    s.box("wheelhouse_roof", (-58, y0 + 40, -38.5), (58, y0 + 43.5, 38.5), "roof")
    s.x2("wing", (56, y0, -14), (CF_W - 2, y0 + 36, 14), "wing")
    s.x2("wing_roof", (56, y0 + 36, -15.5), (CF_W - 1.5, y0 + 38.5, 15.5), "roof")
    add_light(s, CF_W - 1, y0 + 26, -15.5, "side")
    add_light(s, -CF_W + 1, y0 + 26, -15.5, "side")
    add_light(s, CF_W - 1, y0 + 26, 15.5, "side")
    add_light(s, -CF_W + 1, y0 + 26, 15.5, "side")
    s.box("mast", (-1.5, y0 + 43.5, -1.5), (1.5, y0 + 66, 1.5), "mast")
    s.box("mast_yard", (-14, y0 + 58, -1), (14, y0 + 60, 1), "mast")
    add_light(s, 0, y0 + 66, 0, "white")
    s.box("radar", (-16, y0 + 43.5, 20), (16, y0 + 46, 24), "radar")
    # Funnels, a pair on each casing, fore and aft of the wings.
    for zc in (-80.0, 80.0):
        s.x2("funnel", (CF_W - 30, y0, zc - 9), (CF_W - 12, 216, zc + 9), "funnel")
    # Railings round the casings' roofs, liferafts on them.
    for k in range(n):
        za = -CF_CASE_Z + k * seg
        if za < -1 and za + seg > 1 and abs(za) < 120:
            pass
        s.x2("glass_rail", (CF_W - 1.5, y0, za + 1), (CF_W, y0 + 14, za + seg - 1), "glass_rail")
    for zc in (-CF_CASE_Z + 2, CF_CASE_Z - 3.5):
        s.x2("glass_rail_end", (CF_CASE, y0, zc), (CF_W - 1.5, y0 + 14, zc + 1.5), "glass_rail")
    for zc in (-240.0, 220.0):
        s.x2("liferaft", (CF_W - 26, y0, zc), (CF_W - 8, y0 + 12, zc + 20), "liferaft")
    # Bollards at the ends.
    for z in (-half + 150, half - 164):
        s.x2("bollard", (CF_W - 22, CF_DECK, z), (CF_W - 16, CF_DECK + 6, z + 14), "bollard")
    # Cars in four lanes, two lorries.
    k = 0
    for x, zs in ((-66, (-470, -370, -270, 130, 230, 330, 430)), (-22, (-480, -380, 180, 280, 400)),
                  (22, (-330, 210, 310, 420)), (66, (-460, -360, -260, -160, 160, 260, 380))):
        for z in zs:
            add_car(s, x, CF_DECK, z, k)
            k += 1
    add_truck(s, 22, CF_DECK, -170, 1)
    add_truck(s, -22, CF_DECK, -150, 4, length=180)
    livery = {"body": "minecraft:white_concrete", "lower": "minecraft:blue_concrete",
              "stripe": "minecraft:blue_concrete", "door": "minecraft:gray_concrete",
              "roof": "minecraft:blue_concrete"}
    # Rust seats at -24..24 m, eyes 3.6 m up, standing on the car deck: ours at the middle.
    return Model("ship_car_ferry", s.boxes, s.painters, livery, eye=(0.0, m(3.6), 0.0))

# ---------------------------------------------------------------- sea ferry

SF_L, SF_HW = m(135.0), m(24.0) / 2
SF_RAIL = 3.0
SF_W = SF_HW - SF_RAIL
SF_DOWN, SF_DECK = -88.0, 64.0            # 5.5 m under, the main deck 4 m up
SF_BODY_Z = 820.0                          # the tall hull's ends, where the car deck doors are
SF_TOP = 224.0                             # the boat deck, 14 m
SF_TIER_W, SF_TIER_Z = 140.0, 600.0        # the top tier of cabins, longer than Rust's upper box
SF_SUN = 277.0                             # its roof, the sun deck, 17.3 m


def sea_end_deck(c):
    """The open decks at the ends: the deck colour, the ramp's hazard edge."""
    ax = abs(c.x)
    if ax > SF_W:
        return Paint("stripe", 0.25)
    d = SF_L / 2 - abs(c.z)
    if "_tip" in c.box.name:
        if d < SF_RAIL + 4:
            return HAZARD_Y if (math.floor(c.x) + math.floor(d)) % 10 < 5 else hexc("#1d1e21")
        return Paint("door", 0.1 if math.floor(d) % 4 == 0 else -0.05)
    if math.floor(ax) in (60, 61):
        return HAZARD_Y
    return Paint("door", 0.0)


def sf_body_painter(c):
    """The tall hull above the band, in the body colour: a row of openings on the car
    deck, two rows of windows above, the car deck doors in the ends."""
    if c.bottom:
        return UNDER
    if c.top:
        ax = abs(c.x)
        if ax > SF_W - 2:
            return Paint("body", 0.15)
        return Paint("door", -0.05 if math.floor(rel(c)[2]) % 24 == 0 else 0.03)
    iy = math.floor(c.y)
    r, d = rel(c), dims(c)
    top = math.floor(c.box.to[1] - 1e-6)
    if iy == top:
        return Paint("body", 0.18)
    if iy == top - 1:
        return Paint("body", 0.06)
    if iy == math.floor(SF_DECK):
        return Paint("body", -0.3)
    if c.face in ("east", "west"):
        # modules of 41 units, symmetric, so both sides share one texture
        a = r[2] % 41
        g = abs(math.floor(a) + 0.5 - 20.5)
        if 92 <= c.y < 104 and g < 6:
            if iy in (92, 103) or g >= 5:
                return FRAME
            return hexc("#2b2f36")
        for y0, y1 in ((126, 148), (166, 188)):
            if y0 <= c.y < y1 and g < 18:
                if iy in (y0, y1 - 1) or g >= 17:
                    return FRAME
                return glass(c.y, g, iy, top=y1, bottom=y0)
            if iy == y1 and g < 18:
                return Paint("body", -0.18)
        if iy in (115, 116) or iy in (155, 156):
            return Paint("body", -0.08 if iy % 2 else 0.06)
        if iy in (205, 206, 207):
            return Paint("stripe", 0.0)
        if iy == 204:
            return Paint("body", -0.15)
        return Paint("body", 0.0)
    # the ends: the car deck's door, outlined in the stripe colour
    ax = abs(c.x)
    if ax < 64 and c.y < 120:
        if ax >= 60 or iy >= 116:
            return Paint("stripe", 0.0)
        if ax < 1 or iy == 90:
            return Paint("body", -0.3)
        return Paint("body", -0.06 if math.floor(c.y) % 8 else -0.16)
    ring = life_ring(ax - 80, c.y - 100)
    if ring:
        return ring
    if iy in (205, 206, 207):
        return Paint("stripe", 0.0)
    for y0, y1 in ((126, 148), (166, 188)):
        g = abs(math.floor(ax) % 41 + 0.5 - 20.5)
        if y0 <= c.y < y1 and ax < 164 and g < 18:
            if iy in (y0, y1 - 1) or g >= 17:
                return FRAME
            return glass(c.y, g, iy, top=y1, bottom=y0)
        if iy == y1 and ax < 164 and g < 18:
            return Paint("body", -0.18)
    return Paint("body", 0.0)


def sf_tier_painter(c):
    """The top tier of cabins and lounges: a band of windows, doors at the ends."""
    if c.bottom:
        return UNDER
    if c.top:
        return Paint("body", 0.05)
    iy = math.floor(c.y)
    r, d = rel(c), dims(c)
    if iy == math.floor(c.box.frm[1]):
        return Paint("body", -0.3)
    a = r[2] if c.side else r[0]
    if 236 <= c.y < 260:
        if iy in (236, 259) or math.floor(a) % 21 == 0:
            return FRAME
        return glass(c.y, a, iy, top=260, bottom=236)
    if iy == 260:
        return Paint("body", -0.18)
    return Paint("body", 0.0)


def paint_roof_deck(c):
    """A roof slab: the roof colour round its edge, the deck colour on top."""
    if c.bottom:
        return Paint("roof", -0.4)
    if c.top:
        r, d = rel(c), dims(c)
        if r[0] < 2 or r[0] > d[0] - 2:
            return Paint("roof", 0.1)
        if math.floor(abs(c.x)) in (40, 41) and math.floor(r[2]) % 21 < 12:
            return hexc("#e6ecef")
        return Paint("door", 0.04 if math.floor(r[2]) % 21 else -0.06)
    iy = math.floor(c.y)
    return Paint("roof", 0.12 if iy == math.floor(c.box.to[1] - 1e-6) else -0.05)


LIFEBOAT = hexc("#ec6b1c")
LIFEBOAT_LO = hexc("#c4531a")


def paint_lifeboat(c):
    """An enclosed lifeboat: orange hull, white canopy with windows."""
    r, d = rel(c), dims(c)
    iy = math.floor(r[1])
    if c.box.name.startswith("lifeboat_top"):
        if c.top:
            return WHITE if 6 < r[2] < d[2] - 6 else LIFEBOAT
        if c.side and iy in (4, 5) and math.floor(r[2]) % 10 in (3, 4, 5, 6):
            return GLASS_DARK
        return WHITE if iy >= 2 else WHITE_SH
    if c.bottom:
        return LIFEBOAT_LO
    if c.top:
        return LIFEBOAT
    if iy == math.floor(d[1] - 1e-6):
        return mix(LIFEBOAT, (255, 255, 255), 0.25)
    if iy < 4:
        return LIFEBOAT_LO
    return LIFEBOAT


def add_lifeboat(s, x0, x1, y, z0, z1):
    """A lifeboat at the deck's edge, x0 inboard, in its davits."""
    s.box("lifeboat", (x0, y + 4, z0), (x1, y + 22, z1), "lifeboat")
    s.box("lifeboat_top", (x0 + 4, y + 22, z0 + 10), (x1 - 4, y + 34, z1 - 10), "lifeboat")
    for zz in (z0 + 6, z1 - 10):
        s.box("davit", (x0 - 8, y + 38, zz), ((x0 + x1) / 2, y + 42, zz + 4), "steel")


def sea_ferry():
    s = Ship()
    half = SF_L / 2
    s.painters.update({"dark": paint_dark, "steel": paint_steel, "lamp": lamp_painter, "roof": paint_roof,
                       "mast": paint_mast, "white_rail": paint_white_rail, "radar": paint_radar,
                       "funnel": paint_funnel, "sf_body": sf_body_painter, "sf_tier": sf_tier_painter,
                       "roof_deck": paint_roof_deck, "wing": wing_painter(18), "glass_rail": paint_glass_rail,
                       "lifeboat": paint_lifeboat, "bollard": paint_bollard, "liferaft": paint_liferaft,
                       "wheelhouse": wheelhouse_painter(SF_SUN + 18, SF_SUN + 46)})
    style = HullStyle(
        deck=sea_end_deck,
        inner=lambda c, cap=False: Paint("body", 0.1) if cap else Paint("body", -0.12),
        band_y0=54.0, stripe=7, marks=((-half + 86, -half + 90), (half - 90, half - 86)))
    end = dict(pts=[(260.0, SF_W), (200, SF_W - 5), (140, SF_W - 19), (90, SF_W - 43), (46, SF_W - 75), (16, SF_W - 105)],
               tops=[80, 81, 82, 83, 84], bots=[-88, -86, -78, -64, -44], t=8.0, fill_top=SF_DECK,
               tip=(0.0, SF_W - 105, SF_DECK + 3, -30), anchor=3, slab=8.0)
    build_hull(s, style, SF_L, SF_HW, SF_RAIL, SF_DOWN, SF_DECK, end, None, 10, double=True, strip_band=True)
    # The tall hull above the band, as long as the parallel body: car decks inside.
    n = 10
    seg = 2 * SF_BODY_Z / n
    for k in range(n):
        za = -SF_BODY_Z + k * seg
        s.box("sf_body", (-SF_W, SF_DECK, za), (SF_W, SF_TOP, za + seg), "sf_body")
    # The top tier and its roof, the sun deck.
    tl = SF_TIER_Z / 3
    for k in range(6):
        za = -SF_TIER_Z + k * tl
        s.box("tier", (-SF_TIER_W, SF_TOP, za), (SF_TIER_W, 272, za + tl), "sf_tier")
        s.box("sun_deck", (-SF_TIER_W, 272, za), (SF_TIER_W, SF_SUN, za + tl), "roof_deck")
        s.x2("glass_rail", (SF_TIER_W - 1.5, SF_SUN, za + 1), (SF_TIER_W, SF_SUN + 14, za + tl - 1), "glass_rail")
    # Bridges at both ends of the tier, their wings out to the ship's sides.
    bz0, bz1 = -SF_TIER_Z + 4, -SF_TIER_Z + 68
    y0 = SF_SUN
    b = s.box("bridge", (-88, y0, bz0), (88, y0 + 50, bz1), "wheelhouse")
    s.z2(b)
    b = s.box("bridge_roof", (-88, y0 + 50, bz0), (88, y0 + 54, bz1), "roof")
    s.z2(b)
    for side in (1, -1):
        w = s.box("wing", (88, y0, bz0), (SF_W - 1, y0 + 46, bz0 + 30), "wing")
        r = s.box("wing_roof", (88, y0 + 46, bz0), (SF_W - 1, y0 + 49, bz0 + 30), "roof")
        for bb in (w, r):
            if side < 0:
                s.boxes.remove(bb)
                s.boxes.append(bb.mirrored(s._name(bb.name + "_l")))
        for bb in s.boxes[-2:]:
            s.z2(bb)
    for zz in (bz0 + 1.5, -bz0 - 1.5):
        add_light(s, SF_W - 1, y0 + 36, zz, "side")
        add_light(s, -SF_W + 1, y0 + 36, zz, "side")
        s.box("mast", (-1.5, y0 + 54, zz + (24 if zz < 0 else -27)), (1.5, y0 + 80, zz + (27 if zz < 0 else -24)), "mast")
        add_light(s, 0, y0 + 80, zz + (25.5 if zz < 0 else -25.5), "white")
    for zz in (bz1 - 20, -bz1 + 16):
        s.box("radar", (-20, y0 + 54, zz), (20, y0 + 57, zz + 4), "radar")
    # A funnel each side amidships, two exhausts out of each.
    s.x2("funnel", (94, SF_SUN, -40), (138, 360, 40), "funnel")
    for zc in (-16.0, 10.0):
        s.x2("exhaust_pipe", (110, 360, zc), (118, 368, zc + 6), "dark")
    # Lifeboats along the boat deck, both sides, and liferafts.
    for zc in (-330.0, -150.0, 150.0, 330.0):
        add_lifeboat(s, 148, 186, SF_TOP, zc - 60, zc + 60)
        last = s.boxes[-4:]
        s.mirror_x(last)
    for zc in (-460.0, 440.0):
        s.x2("liferaft", (SF_W - 26, SF_TOP, zc), (SF_W - 8, SF_TOP + 12, zc + 20), "liferaft")
    for k in range(n):
        za = -SF_BODY_Z + k * seg
        s.x2("glass_rail", (SF_W - 1.5, SF_TOP, za + 1), (SF_W, SF_TOP + 14, za + seg - 1), "glass_rail")
    # Bollards and winches on the end decks.
    for z in (-half + 170, half - 184):
        s.x2("bollard", (SF_W - 30, SF_DECK, z), (SF_W - 24, SF_DECK + 7, z + 14), "bollard")
    w = s.box("winch", (-30, SF_DECK, -SF_BODY_Z - 34), (30, SF_DECK + 12, -SF_BODY_Z - 18), "steel")
    s.z2(w)
    livery = {"body": "minecraft:white_concrete", "lower": "minecraft:blue_concrete",
              "stripe": "minecraft:blue_concrete", "door": "minecraft:gray_concrete",
              "roof": "minecraft:blue_concrete"}
    # Rust seats at -30..30 m, eyes 18.9 m up: on the sun deck amidships, between the funnels.
    return Model("ship_sea_ferry", s.boxes, s.painters, livery, eye=(0.0, m(18.9), 0.0))

MODELS = [
    car_ferry, sea_ferry,
    lambda: barge("dry"), lambda: barge("dry", True),
    lambda: barge("container"), lambda: barge("container", True),
    lambda: barge("tank"), lambda: barge("tank", True),
    lambda: river_cruise(), lambda: river_cruise(True),
]
