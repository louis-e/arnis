"""Small aircraft: a twin turboprop airliner (ATR 72 class), a light business jet and a
four-seat piston single.

The frame is the vehicles' but for one point: z = 0 is the main landing gear's axle line,
not the middle, since Arnis pitches an aircraft about its main wheels. The nose is at -z,
the right wing at +x, the wheels stand on y = 0.

Livery slots: `body` the fuselage, `lower` its belly (with the fairings and gear pods
under it), `stripe` the cheatline under the windows, `door` the fin and the wingtips,
`roof` the engine nacelles (on the propeller aircraft the cowling and spinner). Wings,
tailplanes, the fairing over a wing's root, struts and gear are fixed colours.

The landing gear of the two retractables is in bones named `gear_*`, which the kit leaves
out of `<id>_up`; the airframe is closed without them.

How the fuselages are built: a rounded cross-section is a few nested boxes, the widest
the lowest. Along the cabin the widest ring is cut into window bays whose faces come out
alike (and the same either way round, so both sides share them), while the other rings run
through as long boxes whose faces do not change along them and squeeze to a strip. The
rider's eye is inside every box of the cabin's cross-section, so none of them is drawn
from the seat; every other box of the fuselage lies inside its neighbour's outline where
they meet, so no bulkhead shows either. A raked pane, a swept fin or a rising tail is
level boxes stepping along the slope under one turned slab or strip.
"""
import math

from kit import (
    CHROME, CHROME_DARK, GLASS_LIGHT, GLASS_MID, TIRE, TIRE_SIDE, TREAD_GROOVE, TREAD_RIB, TRIM,
    Box, Model, Paint, glass, hexc, mix, pair,
)

# ---------------------------------------------------------------- colours

WING = hexc("#e3e6ea")
WING_HI = hexc("#f2f4f6")
WING_LO = hexc("#cdd2d8")
WING_UNDER = hexc("#e2e5e9")
WING_UNDER_LO = hexc("#d3d7dc")
PANEL = hexc("#9299a2")
BOOT = hexc("#27292d")
BOOT_HI = hexc("#3a3d43")
INTAKE = hexc("#141518")
INTAKE_HI = hexc("#2c2f35")
EXHAUST = hexc("#4a4640")
EXHAUST_DARK = hexc("#2a2724")
LEG = hexc("#c2c7cd")
LEG_DARK = hexc("#8d939b")
OLEO = hexc("#e4e8ec")
BLADE = hexc("#2b2d31")
BLADE_FACE = hexc("#3d4046")
BLADE_TIP = hexc("#e9e6dc")
NAV_RED = hexc("#ff3a2e")
NAV_GREEN = hexc("#36e070")
NAV_WHITE = hexc("#fff6dc")
BEACON = hexc("#ff4a36")
LANDING = hexc("#fffbe8")
PANE = hexc("#1c2836")
WELL = hexc("#3b3f46")

LIVERY = {
    "body": "minecraft:white_concrete", "lower": "minecraft:light_gray_concrete",
    "stripe": "minecraft:blue_concrete", "door": "minecraft:blue_concrete",
    "roof": "minecraft:white_concrete",
}


# ---------------------------------------------------------------- building helpers


def rows(y):
    return math.floor(y + 1e-6)


def run(box, n, dz, name=None):
    """n copies of a box, dz apart along the aircraft: bays whose faces come out alike."""
    base = name or box.name
    return [box.moved(dz=k * dz, name=f"{base}_{k}") for k in range(n)]


def raked(name, base, top, levels, half, thick, mat, glass_mat, inset=0.5, **extra):
    """A raked pane from `base` = (z, y) up to `top` = (z, y): level steps of `mat` between
    the heights in `levels`, each reaching from the pane back to top's z, and over them the
    slab of `glass_mat`, `inset` narrower than the steps so that its underside stays
    inside them, out of a rider's view. A windscreen rises towards the tail, a rear window
    towards the nose."""
    (z0, y0), (z1, y1) = base, top
    rise, length = y1 - y0, math.hypot(z1 - z0, y1 - y0)
    angle = math.degrees(math.atan2(rise, abs(z1 - z0)))
    assert angle <= 45.0, (name, angle)
    out = []
    for k in range(len(levels) - 1):
        ya, yb = levels[k], levels[k + 1]
        assert thick >= (yb - ya) * math.cos(math.radians(angle)), (name, "slab too thin")
        zb = z0 + (z1 - z0) * (yb - y0) / rise
        if abs(zb - z1) > 0.05:
            out.append(Box(f"{name}_step_{k}", (-half, ya, min(zb, z1)), (half, yb, max(zb, z1)), mat, **extra))
    hs = half - inset
    if z1 > z0:
        out.append(Box(name, (-hs, y0 - thick, z0), (hs, y0, z0 + length), glass_mat,
                       origin=(0, y0, z0), rot=(-angle, 0, 0), base=z0, **extra))
    else:
        out.append(Box(name, (-hs, y0 - thick, z0 - length), (hs, y0, z0), glass_mat,
                       origin=(0, y0, z0), rot=(angle, 0, 0), base=z0, **extra))
    return out


def raked_pillars(name, base, top, half, thick, mat, proud=0.25, wide=1.0, **extra):
    """The strips over a raked pane's two edges (they cover the steps' corners). A quarter
    deeper than the pane and a quarter short of its foot, so that no face of one lies in
    a face of the other."""
    (z0, y0), (z1, y1) = base, top
    length = math.hypot(z1 - z0, y1 - y0)
    angle = math.degrees(math.atan2(y1 - y0, abs(z1 - z0)))
    low = y0 - thick - 0.25
    if z1 > z0:
        b = Box(name + "_right", (half + proud - wide, low, z0 + 0.25), (half + proud, y0 + 0.3, z0 + length), mat,
                origin=(0, y0, z0), rot=(-angle, 0, 0), **extra)
    else:
        b = Box(name + "_right", (half + proud - wide, low, z0 - length), (half + proud, y0 + 0.3, z0 - 0.25), mat,
                origin=(0, y0, z0), rot=(angle, 0, 0), **extra)
    return pair(b)


def belly_slab(name, z0, z1, y0, y1, half, thick, mat, **extra):
    """A slab whose underside rises from y0 at z0 to y1 at z1: the smooth underside of a
    tail cone, under level boxes that step up behind it."""
    angle = math.degrees(math.atan2(y1 - y0, z1 - z0))
    length = math.hypot(z1 - z0, y1 - y0)
    return Box(name, (-half, y0, z0), (half, y0 + thick, z0 + length), mat,
               origin=(0, y0, z0), rot=(-angle, 0, 0), **extra)


def swept_edge(name, foot, tip, thick, half, mat, below=3.0, **extra):
    """A strip along a fin's swept leading edge from `foot` = (z, y) up to `tip` = (z, y),
    `thick` deep behind the edge, over level slabs that step back behind it."""
    (z0, y0), (z1, y1) = foot, tip
    angle = math.degrees(math.atan2(z1 - z0, y1 - y0))
    assert 0 <= angle <= 45.0, (name, angle)
    length = math.hypot(z1 - z0, y1 - y0)
    return Box(name, (-half, y0 - below, z0), (half, y0 + length, z0 + thick), mat,
               origin=(0, y0, z0), rot=(angle, 0, 0), **extra)


def swept_trail(name, foot, tip, thick, half, mat, below=3.0, above=0.0, **extra):
    """A strip along a fin's trailing edge from `foot` = (z, y) up to `tip` = (z, y), `thick`
    deep ahead of the edge: the rudder, behind level slabs that stop short of the edge."""
    (z0, y0), (z1, y1) = foot, tip
    angle = math.degrees(math.atan2(z1 - z0, y1 - y0))
    assert 0 <= angle <= 45.0, (name, angle)
    length = math.hypot(z1 - z0, y1 - y0)
    return Box(name, (-half, y0 - below, z0 - thick), (half, y0 + length + above, z0), mat,
               origin=(0, y0, z0), rot=(angle, 0, 0), **extra)


def fin_slabs(name, levels, le, te, half, mat, short=0.0, **extra):
    """Level slabs of a swept fin between the heights in `levels`: `le(y)` and `te(y)`
    give the edges; each slab starts where the leading edge is at its top. With `short`
    they end that far ahead of the trailing edge at their foot, for a rudder strip to
    cover; without it they step back along the edge."""
    out = []
    for k in range(len(levels) - 1):
        ya, yb = levels[k], levels[k + 1]
        end = te(ya) - short if short else te(yb)
        out.append(Box(f"{name}_{k}", (-half, ya, le(yb)), (half, yb, end), mat, **extra))
    return out


def line(p0, p1):
    """z as a function of y along the line through p0 = (z, y) and p1 = (z, y)."""
    (z0, y0), (z1, y1) = p0, p1
    return lambda y: z0 + (z1 - z0) * (y - y0) / (y1 - y0)


def wheel(name, x, y, z, r, half, bone, mat="wheel", fat=False):
    """A wheel centred on (x, y, z), radius r, half its width: two boxes in a plus, with
    a third between them when it is big."""
    c = (x, y, z)
    a, b = round(r * 0.56 * 4) / 4, r
    out = [
        Box(f"{name}_crown", (x - half, y - b, z - a), (x + half, y + b, z + a), mat, bone, wheel=c, radius=r),
        Box(f"{name}_tread", (x - half + 0.25, y - a, z - b), (x + half - 0.25, y + a, z + b), mat, bone, wheel=c, radius=r),
    ]
    if fat:
        d = round(r * 0.82 * 4) / 4
        out.append(Box(f"{name}_tyre", (x - half + 0.5, y - d, z - d), (x + half - 0.5, y + d, z + d), mat, bone,
                       wheel=c, radius=r))
    return out


def paint_wheel(c):
    wx, wy, wz = c.box.extra["wheel"]
    r = c.box.extra["radius"]
    if c.side:
        d = math.hypot(c.p[2] - wz, c.p[1] - wy) / r
        if d < 0.3:
            return CHROME_DARK
        if d < 0.55:
            return hexc("#d9dde1")
        return TIRE_SIDE if d < 0.8 else TIRE
    return TREAD_GROOVE if rows(abs(c.p[0] - wx) * 2) % 2 == 1 else TREAD_RIB


def belly_paint(limit, k=-0.05):
    """The painter of a tail's belly slabs: the belly's colour underneath, and on the
    sides what the fuselage wears where the slab starts, the belly's below `limit` and the
    body's above. One colour a slab, since a level edge (or the cheatline) would come out
    ragged on a turned face."""
    def paint(c):
        if c.side and c.box.frm[1] + 1 >= limit:
            return Paint("body", k)
        return Paint("lower", -0.04 if c.side else -0.06)
    return paint


def paint_nav(c):
    return NAV_RED if c.p[0] < 0 else NAV_GREEN


def pane(y, z, y0, y1, z0, z1):
    """A cabin window's dark pane between rows y0..y1 and columns z0..z1 (both
    upper ends excluded), its corners cut, lighter towards the top; None outside it. The
    same either way round, so a bay's two sides share one spot of the texture."""
    iy, iz = rows(y), math.floor(z)
    if not (y0 <= iy < y1 and z0 <= iz < z1):
        return None
    corner = (iy in (y0, y1 - 1)) and (iz in (z0, z1 - 1))
    if corner and (y1 - y0) >= 4 and (z1 - z0) >= 4:
        return None
    if iy == y1 - 1:
        return mix(PANE, GLASS_LIGHT, 0.55)
    if iy == y1 - 2:
        return mix(PANE, GLASS_MID, 0.6)
    return PANE


def ws_paint(c):
    """A raked pane's slab: glass in a dark frame along its foot and head (unless the box
    says `foot` or `head` is open), with posts where `posts` gives ranges of |x|; `lo` and
    `hi` are the heights its tint runs between."""
    x, y = c.p[0], c.p[1]
    e = c.box.extra
    if c.face != "up":
        return TRIM
    d = abs(c.loc[2] - e["base"])
    n = c.box.to[2] - c.box.frm[2]
    if (d < 0.9 and e.get("foot", True)) or (d > n - 0.9 and e.get("head", True)):
        return TRIM
    if any(a <= abs(x) < b for a, b in e.get("posts", ())):
        return TRIM
    return glass(y, x, d, top=e["hi"], bottom=e["lo"])


def wing_paint(c):
    """A wing or tailplane panel, the same all along the span so its faces squeeze to a
    strip: a lighter leading edge, the flap's hinge line and a darker trailing edge. The
    box says where its edges are: `le`, `te` (in its own frame) and `flap`, the depth of
    the flap ahead of the trailing edge."""
    e = c.box.extra
    z = c.loc[2]
    d_le = z - e["le"] if "le" in e else 99.0
    d_te = e["te"] - z if "te" in e else 99.0
    flap = e.get("flap")
    boot = e.get("boot", 0)
    bright = e.get("bright", 0)
    if bright and (d_le < bright or c.face == "north"):
        return CHROME if not c.bottom else CHROME_DARK
    if c.top:
        if d_le < boot:
            return BOOT_HI if d_le < 1 else BOOT
        if d_le < boot + 2:
            return WING_HI
        if flap is not None and rows(d_te) == flap:
            return PANEL
        if d_te < 1:
            return WING_LO
        return WING
    if c.bottom:
        if d_le < boot:
            return BOOT
        if flap is not None and rows(d_te) == flap:
            return PANEL
        return WING_UNDER if d_le < 3 else WING_UNDER_LO
    if c.face == "north":
        return BOOT_HI if boot else WING_HI
    if c.face == "south":
        return WING_LO
    return WING


# ================================================================ plane_light
#
# A four-seat piston single: 8.3 m long, 11.0 m across, 2.7 m high. High wing with one
# strut each side, fixed tricycle gear. The main axle is z 0, the spinner's tip z -44.

LT_AXIS = 19.0            # the propeller's axis
LT_WS = ((-22.0, 25.0), (-13.0, 32.0))      # windscreen: base and top, (z, y)
LT_RW = ((25.0, 27.0), (13.0, 32.0))        # rear window
LT_TAIL = (25.0, 86.0)    # the tail cone
LT_BELLY = (11.0, 21.0)   # its underside at either end
LT_FIN_LE = line((64.0, 26.0), (78.5, 42.5))
LT_FIN_TE = line((86.5, 26.0), (88.5, 42.5))


def lt_stripe_row(z):
    """The row the side stripe's lower edge is in: level along the cabin, a step up with
    each section of the tail cone."""
    n = 5 * (z - LT_TAIL[0]) / (LT_TAIL[1] - LT_TAIL[0])
    return 17 if n < 0 else 18 + min(4, math.floor(n))


def lt_side(y, z, cowl=False):
    iy, iz = rows(y), math.floor(z)
    s = lt_stripe_row(z)
    slot = "roof" if cowl else "body"
    if not cowl and iy >= 24 and z < LT_TAIL[0]:
        # The window band: door glass, the door post, rear side window, quarter window.
        if iy >= 31:
            return TRIM
        if iz in (-2, -1) or iz in (11, 12):
            return Paint("body", 0.04)
        if iz >= 13 and iy == 24:
            return Paint("body", 0.08)
        return glass(y, z, y, top=31.0, bottom=24.0)
    if iy in (s, s + 1) and iy < 24:
        return Paint("stripe", 0.06 if iy == s + 1 else -0.04)
    if iy == 15 and z < LT_TAIL[0]:
        return Paint("stripe", -0.02)
    if iy <= 13:
        return Paint("lower", {13: -0.02, 12: -0.1, 11: -0.18}.get(iy, -0.26))
    if not cowl:
        # The door: a 1-texel outline and a handle.
        if 14 <= iy and (iz in (-19, -2)) or (iy == 14 and -19 <= iz <= -2):
            return Paint("body", -0.34)
        if iy == 21 and iz in (-5, -4):
            return CHROME_DARK
        if iz == 13 and iy < 24:
            return Paint("body", -0.2)
    elif iz == -23:
        return Paint(slot, -0.3)
    if iy >= 23:
        return Paint(slot, 0.12)
    return Paint(slot, 0.02 if iy >= 19 else -0.04)


def lt_fus(c):
    y, z = c.p[1], c.p[2]
    if c.bottom:
        return Paint("lower", -0.1)
    if c.top:
        return Paint("body", 0.1)
    if c.side:
        return lt_side(y, z)
    return Paint("lower", -0.22) if y < 14 else Paint("body", -0.06)


def lt_belly(c):
    if c.side:
        return Paint("lower", -0.14)
    return Paint("lower", -0.1)


def lt_cowl(c):
    x, y, z = c.p
    ax, iy = abs(x), rows(y)
    if c.bottom:
        return Paint("lower", -0.1)
    if c.top:
        if math.floor(z) == -23:
            return Paint("roof", -0.3)
        return Paint("roof", 0.1)
    if c.side:
        return lt_side(y, z, cowl=True)
    if c.front:
        if z < -39.5:
            # The nose bowl: two cooling inlets beside the spinner, the air intake below.
            if 20 <= iy <= 22 and 2.5 <= ax < 6:
                return INTAKE if iy < 22 else INTAKE_HI
            if 14 <= iy <= 15 and ax < 4:
                return INTAKE if iy == 14 else INTAKE_HI
            if iy <= 13:
                return Paint("lower", -0.1)
            return Paint("roof", 0.06 if iy >= 20 else -0.04)
        return Paint("roof", -0.08) if iy > 13 else Paint("lower", -0.16)
    return Paint("roof", -0.1)


def lt_spinner(c):
    if c.front:
        return Paint("roof", 0.14)
    return Paint("roof", 0.04 if c.top else -0.12 if c.bottom else -0.03)


def lt_prop(c):
    d = abs(c.loc[1] - LT_AXIS)
    if d > 13:
        return BLADE_TIP
    if c.face == "north":
        return BLADE_FACE if d > 3 else BLADE
    return BLADE


def lt_fin(c):
    iy = rows(c.p[1])
    if c.box.extra.get("rudder"):
        # The rudder: its hinge line along the strip's front.
        if c.side:
            return Paint("door", -0.3 if c.loc[2] - c.box.frm[2] < 1 else -0.03)
        return Paint("door", -0.1)
    if c.top:
        return Paint("door", 0.12)
    if c.side and not any(c.box.rot):
        return Paint("door", 0.06 if iy >= 41 else 0.0)
    return Paint("door", 0.08 if c.front else -0.06)


def lt_leg(c):
    if c.top or c.front:
        return LEG
    return LEG_DARK


def lt_strut(c):
    return WING_LO if c.face == "south" else WING_HI


def build_light():
    boxes = []
    add = boxes.append

    # Propeller and spinner.
    add(Box("spinner", (-2.5, LT_AXIS - 2.5, -42.25), (2.5, LT_AXIS + 2.5, -40), "spinner"))
    add(Box("spinner_tip", (-1.5, LT_AXIS - 1.5, -44), (1.5, LT_AXIS + 1.5, -42.25), "spinner"))
    add(Box("propeller", (-1.25, LT_AXIS - 15, -41.75), (1.25, LT_AXIS + 15, -41), "prop",
            origin=(0, LT_AXIS, -41.4), rot=(0, 0, 32)))

    # Cowling: a core with a narrower top and chin, and the nose bowl.
    add(Box("cowl_bowl", (-7, 13, -40), (7, 24, -37), "cowl"))
    add(Box("cowl", (-8, 12.5, -37), (8, 24, -22), "cowl"))
    add(Box("cowl_top", (-6, 24, -37), (6, 25, -22), "cowl"))
    add(Box("cowl_chin", (-5.5, 11.5, -36), (5.5, 12.5, -22), "cowl"))
    add(Box("exhaust", (3.5, 9.5, -27.5), (5, 12, -26), "exhaust"))
    add(Box("landing_light", (-1.5, 14.25, -40.3), (1.5, 15.75, -40), "landing", lamp="head"))

    # Fuselage: the cabin is one box the rider sits in; ahead of it the part under the
    # windscreen, behind it the part under the rear window, a keel under all three.
    add(Box("front", (-9, 11, -22), (9, 25, -13), "fus"))
    add(Box("cabin", (-9, 11, -13), (9, 32, 13), "fus"))
    add(Box("rear", (-9, 11, 13), (9, 27, 25), "fus"))
    add(Box("keel", (-6.5, 10, -22), (6.5, 11, 25), "belly"))
    boxes += raked("windscreen", LT_WS[0], LT_WS[1], [25, 27, 29, 31, 32], 9, 1.75, "fus", "windscreen",
                   lo=25.0, hi=32.0)
    boxes += raked_pillars("a_pillar", LT_WS[0], LT_WS[1], 9, 1.75, "pillar")
    boxes += raked("rear_window", LT_RW[0], LT_RW[1], [27, 29, 31, 32], 9, 2.0, "fus", "windscreen",
                   lo=27.0, hi=32.0)
    boxes += raked_pillars("c_pillar", LT_RW[0], LT_RW[1], 9, 2.0, "pillar")

    # Tail cone: level sections stepping in, a slab under each for the rising underside.
    widths = [7.5, 6.0, 4.5, 3.0, 1.75]
    n = len(widths)
    for k, hw in enumerate(widths):
        za = LT_TAIL[0] + (LT_TAIL[1] - LT_TAIL[0]) * k / n
        zb = LT_TAIL[0] + (LT_TAIL[1] - LT_TAIL[0]) * (k + 1) / n
        ya = LT_BELLY[0] + (LT_BELLY[1] - LT_BELLY[0]) * k / n
        yb = LT_BELLY[0] + (LT_BELLY[1] - LT_BELLY[0]) * (k + 1) / n
        add(Box(f"tail_{k}", (-hw, yb, za), (hw, 26, zb), "fus"))
        add(belly_slab(f"tail_belly_{k}", za, zb, ya, yb, hw - 0.25, 2.4, "tail_belly"))

    # Wing: the roof between the roots, a constant inner panel, and an outer panel of two
    # slabs turned a little so that both edges taper to the tip.
    add(Box("roof", (-9, 32, -13), (9, 35, 13), "wing", le=-13, te=13, flap=8))
    boxes += pair(Box("wing_inner_right", (9, 32, -13), (40, 35, 13), "wing", le=-13, te=13, flap=8))
    boxes += pair(Box("wing_outer_right", (40, 32.25, -13), (84.5, 34.75, 3), "wing",
                      origin=(40, 33.5, -13), rot=(0, -2.6, 0), le=-13))
    boxes += pair(Box("aileron_right", (40, 32.5, 1), (84.5, 34.25, 13), "wing",
                      origin=(40, 33.5, 13), rot=(0, 7.8, 0), te=13))
    boxes += pair(Box("wingtip_right", (84, 32, -11.25), (87.5, 35, 7.25), "tip"))
    boxes += pair(Box("nav_right", (87.5, 32.75, -9.5), (88, 34.25, -6.5), "nav", lamp="glow"))
    boxes += pair(Box("strut_right", (8, 12.0, -4), (8 + 41.5, 13.25, -2), "strut",
                      origin=(8, 12.5, -3), rot=(0, 0, 28.3)))

    # Tail: the fin in level slabs under a strip along its swept leading edge and the
    # rudder's strip behind, a dorsal fillet ahead of it, the rudder's foot, the tailplane
    # with turned leading edges.
    boxes += fin_slabs("fin", [26, 30, 34, 38, 42.5], LT_FIN_LE, LT_FIN_TE, 0.75, "fin", short=0.75)
    add(swept_edge("fin_edge", (64.0, 26.0), (78.5, 42.5), 3.25, 1.0, "fin"))
    add(swept_trail("rudder", (86.5, 26.0), (88.5, 42.5), 5.0, 1.0, "fin", below=0.5, above=-0.6, rudder=True))
    add(Box("dorsal", (-0.5, 20.5, 50), (0.5, 26, 66.8), "fin", origin=(0, 26, 50), rot=(-17.4, 0, 0)))
    add(Box("rudder_foot", (-0.6, 21.5, 86), (0.6, 26, 87.25), "fin"))
    add(Box("tailplane", (-27.5, 23.5, 73), (27.5, 25, 85.5), "wing", te=85.5, flap=5))
    boxes += pair(Box("tailplane_edge_right", (1.5, 23.75, 67.5), (28, 24.75, 74.5), "wing",
                      origin=(1.5, 24.25, 67.5), rot=(0, -12.1, 0), le=67.5))
    add(Box("tail_light", (-0.5, 22.5, 87.25), (0.5, 24, 87.75), "white_light", lamp="glow"))
    add(Box("beacon", (-0.5, 42.5, 82), (0.5, 43.5, 84), "beacon", lamp="glow"))
    add(Box("antenna", (-0.25, 35, 3), (0.25, 38, 5.5), "antenna"))

    # Fixed gear: sprung main legs, the nose leg under the firewall.
    boxes += pair(Box("leg_right", (6.5, 10.9, -1.5), (21.5, 12.1, 1.5), "leg",
                      origin=(6.5, 11.5, 0), rot=(0, 0, -33.1)))
    for b in wheel("main_right", 20.0, 3.5, 0.0, 3.5, 1.25, "body"):
        boxes += pair(b)
    boxes += wheel("nose_wheel", 0.0, 3.0, -26.4, 3.0, 1.0, "body")
    boxes += pair(Box("nose_fork_right", (1.0, 2.3, -27.1), (1.6, 7.5, -25.7), "leg"))
    add(Box("nose_leg", (-0.9, 7, -27.2), (0.9, 13, -25.4), "oleo"))

    painters = {
        "fus": lt_fus,
        "belly": lt_belly,
        "tail_belly": belly_paint(14.0, -0.04),
        "cowl": lt_cowl,
        "windscreen": ws_paint,
        "pillar": lambda c: Paint("body", 0.1 if c.face == "up" else 0.0),
        "spinner": lt_spinner,
        "prop": lt_prop,
        "exhaust": lambda c: EXHAUST_DARK if c.bottom else EXHAUST,
        "landing": lambda c: LANDING,
        "wing": wing_paint,
        "tip": lambda c: Paint("door", 0.1 if c.top else -0.1 if c.bottom else 0.0),
        "nav": paint_nav,
        "strut": lt_strut,
        "fin": lt_fin,
        "white_light": lambda c: NAV_WHITE,
        "beacon": lambda c: BEACON,
        "antenna": lambda c: WING_LO,
        "leg": lt_leg,
        "oleo": lambda c: OLEO if rows(c.p[1]) % 3 else LEG_DARK,
        "wheel": paint_wheel,
    }
    return Model("plane_light", boxes, painters, LIVERY, eye=(0.0, 27.5, -7.0))


# ================================================================ plane_bizjet
#
# A light business jet: 15.6 m long, 16.3 m across, 5.1 m high. Low wing with a little
# sweep, two turbofans on the rear fuselage, T-tail. The main axle is z 0, the nose's tip
# z -132, the nose wheel z -100.8.

BZ_BAY0, BZ_PITCH, BZ_BAYS = -60.0, 13.0, 6      # the cabin's window bays
BZ_CABIN = (-78.0, 36.0)                         # the constant section
BZ_WS = ((-98.0, 30.0), (-89.0, 36.0))           # windscreen: base and top, (z, y)
BZ_RINGS = [(14.0, 18.0, 32.0), (12.0, 14.0, 36.0), (8.0, 12.0, 38.0), (4.5, 11.0, 39.0)]
BZ_TAIL = (36.0, 108.0)
BZ_BELLY = (12.0, 30.0)
BZ_FIN_LE = line((50.0, 38.0), (88.0, 77.0))
BZ_FIN_TE = line((100.0, 38.0), (111.0, 77.0))
BZ_ENGINE = (24.5, 33.5)                         # a nacelle's axis: x, y


def bz_tone(iy):
    """The fuselage's paint by height: belly, cheatline, body."""
    if iy <= 16:
        return Paint("lower", 0.05 if iy == 16 else -0.04 if iy >= 13 else -0.12)
    if iy in (21, 22):
        return Paint("stripe", 0.06 if iy == 22 else -0.04)
    if iy <= 20:
        return Paint("body", -0.06)
    if iy >= 36:
        return Paint("body", 0.12)
    return Paint("body", 0.05 if iy >= 30 else 0.0)


def bz_fus(c):
    x, y, z = c.p
    iy, iz = rows(y), math.floor(z)
    e = c.box.extra
    if c.bottom:
        return Paint("lower", -0.06)
    if c.top:
        return Paint("body", 0.12) if y > 17 else Paint("lower", 0.05)
    if not c.side:
        return bz_tone(iy)
    if e.get("bay"):
        w = pane(y, (z - BZ_BAY0) % BZ_PITCH, 24, 30, 4, 9)
        if w:
            return w
    elif e.get("door"):
        if x < 0:
            # The cabin door on the left: a 1-texel outline and a handle.
            if 14 <= iy <= 33 and -75 <= iz <= -63:
                if iy in (14, 33) or iz in (-75, -63):
                    return Paint("body", -0.36) if iy > 16 else Paint("lower", -0.36)
                if iy == 23 and iz in (-66, -65):
                    return CHROME_DARK
        else:
            w = pane(y, z, 24, 30, -71, -66)
            if w:
                return w
    elif e.get("cockpit"):
        # The cockpit's side window, back from the windscreen's pillar.
        if iy == 30 or iy == 35 or iz >= -81:
            return Paint("body", 0.08 if iy == 35 else 0.03)
        return glass(y, z, y, top=35.0, bottom=31.0)
    return bz_tone(iy)


def bz_fairing(c):
    if c.bottom:
        return Paint("lower", -0.06)
    if c.top:
        return Paint("lower", 0.1)
    return Paint("lower", 0.02 if rows(c.p[1]) > 10 else -0.12)


def bz_nacelle(c):
    x, y, z = c.p
    ax, ay = BZ_ENGINE
    dx, dy = abs(abs(x) - ax), abs(y - ay)
    iz = math.floor(z)
    kind = c.box.extra.get("part")
    if c.front:
        if kind == "intake":
            r = max(dx, dy)
            if r > 5:
                return CHROME
            if r < 1:
                return CHROME_DARK
            return INTAKE if r > 2 else INTAKE_HI
        return Paint("roof", -0.04)
    if c.back:
        if kind == "exhaust":
            return EXHAUST_DARK if max(dx, dy) < 3.5 else EXHAUST
        return Paint("roof", -0.14)
    if kind == "exhaust":
        return mix(EXHAUST, (255, 255, 255), 0.12) if c.top else EXHAUST
    if kind == "intake" and iz == 17:
        return CHROME
    if iz == 45:
        return Paint("roof", -0.3)
    if c.top:
        return Paint("roof", 0.12)
    if c.bottom:
        return Paint("roof", -0.06)
    return Paint("roof", 0.05 if y > ay + 2 else -0.08 if y < ay - 3 else 0.0)


def bz_fin(c):
    if c.box.extra.get("edge"):
        # The strip along the leading edge: bright metal or a de-icing boot in front, the
        # fin's colour behind.
        boot = c.box.extra.get("boot")
        if c.face == "north":
            return BOOT_HI if boot else CHROME
        if c.side and c.loc[2] - c.box.frm[2] < 1:
            return BOOT if boot else CHROME_DARK
        return Paint("door", 0.03)
    if c.box.extra.get("rudder"):
        # The rudder: its hinge line along the strip's front.
        if c.side:
            if c.loc[2] - c.box.frm[2] < 1:
                return Paint("door", -0.3)
            return Paint("door", -0.03)
        return Paint("door", -0.12)
    if c.top:
        return Paint("door", 0.12)
    if c.side:
        return Paint("door", 0.0)
    return Paint("door", 0.08 if c.front else -0.08)


def bz_bullet(c):
    if c.top:
        return Paint("door", 0.14)
    if c.front:
        return CHROME
    return Paint("door", 0.04 if c.side else -0.1)


def bz_tip(c):
    return Paint("door", 0.1 if c.top else -0.05 if c.bottom else 0.02)


def bz_gear(c):
    y = c.p[1]
    if c.box.extra.get("door"):
        return WING_LO if c.side and c.n[0] * c.p[0] > 0 else WELL
    if c.box.extra.get("oleo"):
        return OLEO if rows(y) % 3 else LEG_DARK
    return LEG if c.top or c.front else LEG_DARK


def bz_rings(name, z0, z1, top=None, **extra):
    """The fuselage's stepped cross-section between z0 and z1, cut off level at `top`.
    What `extra` says goes to the two outer rings, whose sides show."""
    out = []
    for k, (w, y0, y1) in enumerate(BZ_RINGS):
        y1 = min(y1, top) if top else y1
        out.append(Box(f"{name}_{'abcd'[k]}", (-w, y0, z0), (w, y1, z1), "fus", **(extra if k < 2 else {})))
    return out


def build_bizjet():
    boxes = []
    add = boxes.append

    # Nose: sections stepping down to the tip, the three biggest with a narrower, taller
    # box (a quarter further back, so that the two fronts never lie in one plane).
    nose = [(-132, -129, 1.75, 19.5, 22.5, 0), (-129, -125.5, 3.5, 18.25, 24.25, 0),
            (-125.5, -121.5, 5.25, 17.25, 25.75, 0), (-121.5, -117, 7.0, 16.25, 27.0, 0),
            (-117, -112, 8.5, 16.0, 27.25, 0.75), (-112, -106, 10.0, 15.25, 28.0, 1.0),
            (-106, -98, 12.0, 14.0, 30.0, 1.5)]
    for k, (z0, z1, w, y0, y1, r) in enumerate(nose):
        last = k == len(nose) - 1
        add(Box(f"nose_{k}", (-w, y0, z0), (w, y1, z1), "fus"))
        if r:
            add(Box(f"nose_{k}_tall", (-w * 0.62, y0 - r, z0 + 0.25), (w * 0.62, y1 - 0.5 if last else y1 + r, z1), "fus"))

    # Cockpit: the body up to the windscreen's foot, the glasshouse on it behind the
    # raked windscreen, and the roof's two steps.
    boxes += bz_rings("cockpit", -98, BZ_CABIN[0], top=30.0)
    boxes += raked("windscreen", BZ_WS[0], BZ_WS[1], [30, 32, 34, 36], 12, 2.0, "fus", "windscreen",
                   cockpit=True, posts=((0, 0.5),), lo=30.0, hi=36.0)
    boxes += raked_pillars("windscreen_pillar", BZ_WS[0], BZ_WS[1], 12, 2.0, "pillar")
    add(Box("glasshouse", (-12, 30, BZ_WS[1][0]), (12, 36, BZ_CABIN[0]), "fus", cockpit=True))
    add(Box("brow", (-8, 36, BZ_WS[1][0]), (8, 38, BZ_CABIN[0]), "fus"))
    add(Box("brow_top", (-4.5, 38, -86), (4.5, 39, BZ_CABIN[0]), "fus"))

    # Cabin: the door's section, then the window bays as the widest ring's repeated
    # boxes over three long ones, and a plain section between the engines.
    boxes += bz_rings("door", BZ_CABIN[0], BZ_BAY0, door=True)
    w, y0, y1 = BZ_RINGS[0]
    boxes += run(Box("bay", (-w, y0, BZ_BAY0), (w, y1, BZ_BAY0 + BZ_PITCH), "fus", bay=True), BZ_BAYS, BZ_PITCH)
    aft = BZ_BAY0 + BZ_BAYS * BZ_PITCH
    add(Box("aft_a", (-w, y0, aft), (w, y1, BZ_CABIN[1]), "fus"))
    for k, (w, y0, y1) in enumerate(BZ_RINGS[1:]):
        add(Box(f"cabin_{'bcd'[k]}", (-w, y0, BZ_BAY0), (w, y1, BZ_CABIN[1]), "fus"))

    # Tail cone: level sections stepping in under a spine, each inside the one before, a
    # slab under each for the rising underside.
    widths = [12.0, 10.5, 8.5, 6.5, 4.5]
    n = len(widths)
    add(Box("spine", (-4.0, 36, BZ_TAIL[0]), (4.0, 39, 100), "fus"))
    add(Box("shoulder", (-7.5, 36, BZ_TAIL[0]), (7.5, 38, 66), "fus"))
    for k, hw in enumerate(widths):
        za = BZ_TAIL[0] + (BZ_TAIL[1] - BZ_TAIL[0]) * k / n
        zb = BZ_TAIL[0] + (BZ_TAIL[1] - BZ_TAIL[0]) * (k + 1) / n
        ya = BZ_BELLY[0] + (BZ_BELLY[1] - BZ_BELLY[0]) * k / n
        yb = BZ_BELLY[0] + (BZ_BELLY[1] - BZ_BELLY[0]) * (k + 1) / n
        keel = round(hw * 0.65 * 4) / 4
        add(Box(f"tail_{k}", (-hw, yb + 3, za), (hw, 36, zb), "fus"))
        add(Box(f"tail_keel_{k}", (-keel, yb, za), (keel, yb + 3, zb), "fus"))
        add(belly_slab(f"tail_belly_{k}", za, zb, ya, yb, keel - 0.25, 4.25, "belly"))

    # Wing: strips turned along the swept leading and the trailing edge, two level panels
    # filling in between them at the root, the fairing under the fuselage.
    span = 117.5
    a_le = math.degrees(math.atan2(19.0, span))
    a_te = math.degrees(math.atan2(8.0, span))
    boxes += pair(Box("wing_lead_right", (12, 11.5, -27), (12 + span / math.cos(math.radians(a_le)), 14, -13.5), "wing",
                      origin=(12, 12.75, -27), rot=(0, -a_le, 0), le=-27, bright=1))
    boxes += pair(Box("wing_trail_right", (12, 11.75, 0.5), (12 + span / math.cos(math.radians(a_te)), 13.75, 14), "wing",
                      origin=(12, 12.75, 14), rot=(0, a_te, 0), te=14, flap=6))
    boxes += pair(Box("wing_root_right", (12, 11.0, -21), (41, 14.5, 11), "wing"))
    boxes += pair(Box("wing_mid_right", (41, 11.25, -17), (72, 14.25, 9), "wing"))
    boxes += pair(Box("wingtip_right", (125, 11.25, -9.0), (129.9, 14.25, 6.75), "tip"))
    boxes += pair(Box("nav_right", (129.9, 12, -7.5), (130.4, 13.5, -4.5), "nav", lamp="glow"))
    add(Box("fairing", (-12, 9.5, -34), (12, 14, 24), "fairing"))
    boxes += pair(Box("fairing_right", (12, 9.75, -33.75), (15, 15.25, 23.75), "fairing"))
    boxes += pair(Box("landing_light_right", (10, 11.5, -34.3), (13.5, 13.5, -34), "landing", lamp="head"))

    # Engines: a nacelle of two crossed boxes, the intake ring ahead, the nozzle behind,
    # on a stub pylon.
    ax, ay = BZ_ENGINE
    boxes += pair(Box("nacelle_wide_right", (ax - 7, ay - 5, 22), (ax + 7, ay + 5, 52), "nacelle"))
    boxes += pair(Box("nacelle_tall_right", (ax - 5, ay - 7, 22.5), (ax + 5, ay + 7, 51.5), "nacelle"))
    boxes += pair(Box("intake_right", (ax - 6, ay - 6, 17), (ax + 6, ay + 6, 22), "nacelle", part="intake"))
    boxes += pair(Box("nozzle_right", (ax - 4.5, ay - 4.5, 52), (ax + 4.5, ay + 4.5, 58), "nacelle", part="exhaust"))
    boxes += pair(Box("pylon_right", (11, ay - 2, 26), (ax - 5, ay + 2, 50), "pylon"))

    # Tail: the fin in level slabs under a strip along its swept leading edge, the
    # tailplane on top as two turned strips each side of a centre panel, in a bullet.
    boxes += fin_slabs("fin", [38, 46, 54, 62, 70, 77.5], BZ_FIN_LE, BZ_FIN_TE, 1.0, "fin", short=1.0)
    add(swept_edge("fin_edge", (50.0, 38.0), (88.0, 77.0), 6.0, 1.25, "fin", below=4.0, edge=True))
    add(swept_trail("rudder", (100.0, 38.0), (111.0, 77.0), 7.0, 1.25, "fin", below=1.0, rudder=True))
    add(Box("tail_tip", (-2.5, 32.5, BZ_TAIL[1]), (2.5, 37, BZ_TAIL[1] + 4.5), "fus"))
    add(Box("dorsal", (-0.75, 31, 30), (0.75, 39, 54), "fin", origin=(0, 39, 30), rot=(-14, 0, 0)))
    add(Box("bullet", (-2.25, 76.5, 84), (2.25, 80.5, 116.5), "bullet"))
    a1 = math.degrees(math.atan2(16.0, 47.0))
    a2 = math.degrees(math.atan2(5.6, 49.0))
    boxes += pair(Box("tailplane_lead_right", (3.1, 77.75, 88), (3.1 + 47 / math.cos(math.radians(a1)), 79.25, 97.5), "wing",
                      origin=(3.1, 78.5, 88), rot=(0, -a1, 0), le=88, bright=1))
    boxes += pair(Box("tailplane_trail_right", (1, 78, 103), (1 + 49 / math.cos(math.radians(a2)), 79, 112), "wing",
                      origin=(1, 78.5, 112), rot=(0, -a2, 0), te=112, flap=4))
    add(Box("tailplane_centre", (-25.5, 77.5, 96.5), (25.5, 79.5, 106), "wing"))
    add(Box("beacon", (-0.75, 80.5, 98), (0.75, 81.6, 100.5), "beacon", lamp="glow"))
    add(Box("tail_light", (-0.75, 78, 116.5), (0.75, 79.5, 117.1), "white_light", lamp="glow"))
    add(Box("antenna", (-0.25, 39, -40), (0.25, 42.5, -37), "antenna"))
    add(Box("antenna_belly", (-0.25, 6.5, -50), (0.25, 11, -47.5), "antenna"))

    # Landing gear, all of it in gear_* bones: the airframe is closed without them.
    nz = -100.8
    boxes += wheel("gear_nose_wheel", 0.0, 3.5, nz, 3.5, 1.0, "gear_nose")
    boxes += pair(Box("gear_nose_fork_right", (1.0, 2.8, nz - 0.9), (1.7, 8.5, nz + 0.9), "gear", "gear_nose"))
    add(Box("gear_nose_leg", (-1.0, 8, nz - 1.0), (1.0, 13.5, nz + 1.0), "gear", "gear_nose", oleo=True))
    boxes += pair(Box("gear_nose_door_right", (2.6, 8.5, nz - 4.5), (2.9, 12.9, nz + 5.5), "gear", "gear_nose", door=True))
    for b in wheel("gear_main_right", 36.0, 4.5, 0.0, 4.5, 1.5, "gear_main"):
        boxes += pair(b)
    boxes += pair(Box("gear_main_leg_right", (33.0, 3.5, -1.25), (34.5, 11.5, 1.25), "gear", "gear_main", oleo=True))
    boxes += pair(Box("gear_main_door_right", (32.2, 5.5, -4.5), (32.5, 11.0, 4.5), "gear", "gear_main", door=True))

    painters = {
        "fus": bz_fus,
        "belly": belly_paint(17.0, -0.06),
        "windscreen": ws_paint,
        "pillar": lambda c: Paint("body", 0.12 if c.face == "up" else 0.03),
        "wing": wing_paint,
        "tip": bz_tip,
        "fairing": bz_fairing,
        "nav": paint_nav,
        "landing": lambda c: LANDING,
        "nacelle": bz_nacelle,
        "pylon": lambda c: Paint("body", 0.1 if c.top else -0.06),
        "fin": bz_fin,
        "bullet": bz_bullet,
        "beacon": lambda c: BEACON,
        "white_light": lambda c: NAV_WHITE,
        "antenna": lambda c: WING_LO,
        "gear": bz_gear,
        "wheel": paint_wheel,
    }
    return Model("plane_bizjet", boxes, painters, LIVERY, eye=(0.0, 28.0, -40.5))


# ================================================================ plane_regional
#
# A twin turboprop airliner: 27.2 m long, 27.05 m across, 7.65 m high. Straight high wing
# on top of the fuselage, a nacelle under each wing, the main gear in pods on the fuselage
# sides, T-tail. The main axle is z 0, the nose's tip z -210, the nose wheels z -172.8.

RG_RINGS = {"a": (23.0, 30.0, 48.0), "b": (19.5, 24.0, 54.0), "c": (14.0, 19.0, 59.0), "d": (9.0, 16.0, 62.0)}
RG_BAY0, RG_PITCH, RG_BAYS = -140.0, 24.0, 10    # window bays, two windows each
RG_FRONT = -172.0                                # where the full cross-section starts
RG_AFT = (100.0, 120.0, 137.0)                   # the rear doors' section, the plain one behind
RG_WS = ((-192.0, 46.0), (-192.0 + 52.0 / 3.0, 59.0))   # windscreen: base and top, (z, y)
RG_WS_MID = (-192.0 + 52.0 / 3.0 * 8.0 / 13.0, 54.0)    # where it narrows
RG_BELLY = line((16.0, 120.0), (48.0, 222.0))    # the tail's underside: y by z
RG_FIN_LE = line((150.0, 62.0), (187.0, 117.5))
RG_FIN_TE = line((213.0, 57.0), (222.0, 117.5))
RG_ENGINE = (64.8, 50.0)                         # a propeller's axis: x, y
RG_PROP = 31.5                                   # its radius


def rg_tone(iy):
    """The fuselage's paint by height: belly, cheatline, body."""
    if iy < 30:
        return Paint("lower", 0.05 if iy == 29 else -0.03 if iy >= 20 else -0.1)
    if iy in (37, 38, 39):
        return Paint("stripe", 0.06 if iy == 39 else -0.05 if iy == 37 else 0.0)
    if iy <= 36:
        return Paint("body", -0.05)
    if iy >= 54:
        return Paint("body", 0.12)
    return Paint("body", 0.05 if iy >= 48 else 0.0)


def door_outline(iy, iz, y0, y1, z0, z1):
    """Whether a texel is on the 1-texel outline of a door from rows y0..y1 and columns
    z0..z1 (all inclusive)."""
    return y0 <= iy <= y1 and z0 <= iz <= z1 and (iy in (y0, y1) or iz in (z0, z1))


def rg_fus(c):
    x, y, z = c.p
    iy, iz = rows(y), math.floor(z)
    e = c.box.extra
    if c.bottom:
        return Paint("lower", -0.06) if y < 31 else Paint("body", -0.1)
    if c.top:
        return Paint("body", 0.12) if y > 31 else Paint("lower", 0.05)
    if not c.side:
        return rg_tone(iy)
    dark = Paint("body", -0.36) if iy >= 30 else Paint("lower", -0.36)
    if e.get("bay"):
        w = pane(y, (z - RG_BAY0) % 12.0, 41, 47, 4, 8)
        if w:
            return w
    elif e.get("fwd"):
        # The cockpit's side windows, then the cargo door on the left, a service door on
        # the right.
        if iz <= -165 and 48 <= iy <= 52:
            if iz in (-177, -176):
                return Paint("body", 0.05)
            return glass(y, z, y, top=53.0, bottom=48.0)
        if x < 0:
            if door_outline(iy, iz, 26, 52, -159, -143):
                return dark
            if iy == 34 and iz in (-146, -145):
                return CHROME_DARK
        else:
            if door_outline(iy, iz, 30, 50, -156, -147):
                return dark
            w = pane(y, z, 42, 46, -153, -149)
            if w:
                return w
    elif e.get("aft"):
        # The rear doors, the same on both sides and either way round.
        if door_outline(iy, iz, 25, 51, 104, 115):
            return dark
        w = pane(y, z, 42, 46, 108, 112)
        if w:
            return w
        if iy == 35 and iz in (109, 110):
            return CHROME_DARK
    return rg_tone(iy)


def rg_nacelle(c):
    x, y, z = c.p
    dx = abs(abs(x) - RG_ENGINE[0])
    iy, iz = rows(y), math.floor(z)
    part = c.box.extra.get("part")
    if part == "nozzle":
        if c.back:
            return EXHAUST_DARK if dx < 2 and 48.5 < y < 52.5 else EXHAUST
        return mix(EXHAUST, (255, 255, 255), 0.1) if c.top else EXHAUST
    if c.front:
        if z < -61 and 43 <= iy <= 45 and dx < 4.5:
            return INTAKE if iy < 45 else INTAKE_HI
        return Paint("roof", 0.06)
    if c.back:
        return Paint("roof", -0.14)
    if c.top:
        return Paint("roof", 0.12)
    if c.bottom:
        return Paint("roof", -0.08)
    if iz in (-45, -24, 9):
        return Paint("roof", -0.28)
    if iy in (47, 48) and -38 <= iz <= -34:
        return INTAKE_HI
    return Paint("roof", 0.05 if iy >= 54 else -0.08 if iy <= 44 else 0.0)


def rg_spinner(c):
    if c.front:
        return Paint("roof", 0.16)
    return Paint("roof", 0.08 if c.top else -0.12 if c.bottom else 0.0)


def prop_paint(c):
    """A propeller's slab through the hub, or an upright one from the hub out (`hub` says
    which end the hub is at): dark blades with pale tips."""
    b = c.box
    hub = b.extra.get("hub")
    if hub:
        r = b.to[1] - b.frm[1]
        d = c.loc[1] - b.frm[1] if hub < 0 else b.to[1] - c.loc[1]
    else:
        d = max(abs(c.loc[0] - (b.frm[0] + b.to[0]) / 2), abs(c.loc[1] - (b.frm[1] + b.to[1]) / 2))
        r = max(b.to[0] - b.frm[0], b.to[1] - b.frm[1]) / 2
    if d > r - 2.5:
        return BLADE_TIP
    if c.face == "north":
        return BLADE_FACE
    return BLADE


def rg_pod(c):
    x, y = c.p[0], c.p[1]
    iy = rows(y)
    if c.bottom:
        # The main gear's doors: their hinge lines along each pod, the same all along it so
        # that the face squeezes to a strip.
        if c.box.extra.get("doors"):
            ax = math.floor(abs(x))
            if ax in (22, 31):
                return Paint("lower", -0.4)
            if 22 < ax < 31:
                return Paint("lower", -0.14)
        return Paint("lower", -0.06)
    if c.top:
        return Paint("lower", 0.1)
    if c.front or c.back:
        return Paint("lower", 0.03 if iy >= 20 else -0.06)
    return Paint("lower", 0.06 if iy >= 26 else -0.03 if iy >= 18 else -0.12)


def rg_fairing(c):
    if c.top:
        return WING_HI
    if c.bottom:
        return WING_UNDER_LO
    return WING if rows(c.p[1]) >= 60 else WING_LO


def build_regional():
    boxes = []
    add = boxes.append

    def ring(name, key, z0, z1, **extra):
        w, y0, y1 = RG_RINGS[key]
        return Box(f"{name}_{key}", (-w, y0, z0), (w, y1, z1), "fus", **extra)

    # Nose: sections stepping down to the tip, the bigger ones of nested boxes (each a
    # quarter further back, so that their fronts never lie in one plane).
    nose = [
        (-210, -207, [(4, 32.5, 37.5)]),
        (-207, -203, [(7.25, 30, 40.5)]),
        (-203, -198, [(12, 32, 41.5), (7.5, 27.5, 43)]),
        (-198, -192, [(15.5, 31.5, 44), (10.5, 25, 45)]),
        (-192, -184, [(19.5, 31, 46), (14.5, 24.5, 45.75), (9, 20, 45.5)]),
        (-184, RG_FRONT, [(21, 30, 46), (17.5, 24, 45.75), (12, 19, 45.5), (8, 17.25, 45.25)]),
    ]
    for k, (z0, z1, parts) in enumerate(nose):
        for j, (w, y0, y1) in enumerate(parts):
            end = z1 if z1 == RG_FRONT else z1 + 1.0
            add(Box(f"nose_{k}_{j}", (-w, y0, z0 + 0.25 * j), (w, y1, end), "fus"))

    # Cockpit: the windscreen in one plane, wide below and narrower above, over steps;
    # behind it the glasshouse, then the full cross-section.
    boxes += raked("windscreen", RG_WS[0], RG_WS_MID, [46, 48, 50, 52, 54], 19.5, 2.0, "fus", "windscreen",
                   fwd=True, posts=((0, 0.5), (9, 10)), head=False, lo=46.0, hi=59.0)
    boxes += raked_pillars("windscreen_pillar", RG_WS[0], RG_WS_MID, 19.5, 2.0, "pillar")
    add(Box("glasshouse", (-19.5, 46, RG_WS_MID[0]), (19.5, 54, RG_FRONT), "fus", fwd=True))
    boxes += raked("windscreen_top", RG_WS_MID, RG_WS[1], [54, 56.5, 59], 14, 2.25, "fus", "windscreen",
                   posts=((0, 0.5),), foot=False, lo=46.0, hi=59.0)

    # Fuselage: the widest ring carries the windows, in bays of two; the others run through.
    add(ring("fwd", "a", RG_FRONT, RG_BAY0, fwd=True))
    add(ring("fwd", "b", RG_FRONT, RG_BAY0, fwd=True))
    w, y0, y1 = RG_RINGS["a"]
    boxes += run(Box("bay", (-w, y0, RG_BAY0), (w, y1, RG_BAY0 + RG_PITCH), "fus", bay=True), RG_BAYS, RG_PITCH)
    add(ring("cabin", "b", RG_BAY0, RG_AFT[0]))
    add(ring("doors", "a", RG_AFT[0], RG_AFT[1], aft=True))
    add(ring("doors", "b", RG_AFT[0], RG_AFT[1], aft=True))
    add(ring("aft", "a", RG_AFT[1], RG_AFT[2] + 0.5))
    add(ring("aft", "b", RG_AFT[1], RG_AFT[2] + 0.25))
    add(Box("brow", (-14, 54, RG_WS[1][0]), (14, 59, RG_FRONT), "fus"))
    add(ring("cabin", "c", RG_FRONT, RG_AFT[2]))
    add(ring("cabin", "d", RG_FRONT, 120))

    # Tail: sections of nested boxes stepping in and up, each inside the one before, a
    # slab under each for the rising underside; a spine along the top.
    tail = [
        (137, 154, [(19, 35.5, 47.75), (15.5, 31, 53.5), (10.5, None, 58.5)]),
        (154, 171, [(15, 39, 53), (10, None, 58.25)]),
        (171, 188, [(13, 43, 52.75), (8, None, 58)]),
        (188, 205, [(10, 46.5, 52.5), (6, None, 57.75)]),
        (205, 222, [(5, None, 57.5)]),
    ]
    add(belly_slab("tail_belly_keel", 120, 137, RG_BELLY(120), RG_BELLY(137), 8.75, 5.5, "belly"))
    for k, (z0, z1, parts) in enumerate(tail):
        for j, (w, y0, y1) in enumerate(parts):
            add(Box(f"tail_{k}_{j}", (-w, RG_BELLY(z1) if y0 is None else y0, z0), (w, y1, z1 + 0.25 * j), "fus"))
        add(belly_slab(f"tail_belly_{k}", z0, z1, RG_BELLY(z0), RG_BELLY(z1), parts[-1][0] - 0.25, 5.5, "belly"))
    add(Box("spine", (-8.5, 58, 120), (8.5, 62, 171), "fus"))
    add(Box("spine_aft", (-5.5, 57, 171), (5.5, 61, 205), "fus"))

    # Wing: a fairing over the fuselage, a constant inner panel each side, an outer panel of
    # two slabs turned a little so that both edges taper to the tip. De-icing boots.
    add(Box("wing_fairing", (-14, 59, -38), (14, 66.5, 34), "fairing"))
    boxes += pair(Box("wing_root_right", (14, 54, -32), (19.5, 66, 28), "fairing"))
    boxes += pair(Box("wing_inner_right", (14, 58, -23), (90, 65, 18), "wing", le=-23, te=18, flap=10, boot=2))
    reach = 215.5 - 90
    a_le = math.degrees(math.atan2(6.0, reach))
    a_te = math.degrees(math.atan2(9.5, reach))
    boxes += pair(Box("wing_outer_right", (90, 58.5, -23), (90 + reach / math.cos(math.radians(a_le)), 64.5, 2), "wing",
                      origin=(90, 61.5, -23), rot=(0, -a_le, 0), le=-23, boot=2))
    boxes += pair(Box("wing_flap_right", (90, 59.25, -6), (90 + reach / math.cos(math.radians(a_te)), 63.75, 18), "wing",
                      origin=(90, 61.5, 18), rot=(0, a_te, 0), te=18, flap=8))
    boxes += pair(Box("wingtip_right", (210, 58.25, -17.75), (215.9, 64.75, 9.25), "tip"))
    boxes += pair(Box("nav_right", (215.9, 60.5, -16), (216.4, 62.5, -12), "nav", lamp="glow"))

    # Nacelles: spinner, six blades as three slabs through the hub, the cowling with its
    # chin intake ahead of the wing, deeper under the wing, the jet pipe behind.
    ex, ey = RG_ENGINE
    for b in (
        Box("spinner_right", (ex - 3.5, ey - 3.5, -68), (ex + 3.5, ey + 3.5, -62), "spinner"),
        Box("spinner_tip_right", (ex - 2, ey - 2, -73), (ex + 2, ey + 2, -68), "spinner"),
        Box("prop_up_right", (ex - 1.5, ey, -65.5), (ex + 1.5, ey + RG_PROP, -64.75), "prop", hub=-1),
        Box("prop_down_right", (ex - 1.5, ey - RG_PROP, -65.5), (ex + 1.5, ey, -64.75), "prop", hub=1),
        Box("prop_b_right", (ex - RG_PROP, ey - 1.5, -65.25), (ex + RG_PROP, ey + 1.5, -64.5), "prop",
            origin=(ex, ey, -65), rot=(0, 0, 30)),
        Box("prop_c_right", (ex - RG_PROP, ey - 1.5, -65.0), (ex + RG_PROP, ey + 1.5, -64.25), "prop",
            origin=(ex, ey, -65), rot=(0, 0, -30)),
        Box("cowl_right", (ex - 6.5, 42, -62), (ex + 6.5, 57, -23), "nacelle"),
        Box("cowl_top_right", (ex - 4.5, 57, -60), (ex + 4.5, 58.5, -20), "nacelle"),
        Box("nacelle_right", (ex - 7.5, 41, -22.5), (ex + 7.5, 60, 10), "nacelle"),
        Box("nacelle_aft_right", (ex - 6, 44.5, 10), (ex + 6, 59, 22), "nacelle"),
        Box("jet_pipe_right", (ex - 3, 47, 22), (ex + 3, 54, 27), "nacelle", part="nozzle"),
    ):
        boxes += pair(b)

    # The main gear's pods on the fuselage sides, joined under the belly.
    add(Box("belly_fairing", (-19.5, 14.5, -42), (19.5, 24, 42), "pod"))
    boxes += pair(Box("pod_right", (19.5, 14.5, -30), (33.5, 28, 28), "pod", doors=True))
    boxes += pair(Box("pod_nose_right", (19.5, 16.5, -42), (30.5, 27, -30), "pod"))
    boxes += pair(Box("pod_tail_right", (19.5, 16.5, 28), (30.5, 27, 42), "pod"))
    boxes += pair(Box("landing_light_right", (23, 20, -42.3), (27, 23, -42), "landing", lamp="head"))

    # Tail: the fin in level slabs under strips along its edges, the long dorsal fin, the
    # tailplane on top in a fairing.
    boxes += fin_slabs("fin", [57, 69, 81, 93, 105, 117.5], RG_FIN_LE, RG_FIN_TE, 1.5, "fin", short=1.5)
    add(swept_edge("fin_edge", (150.0, 62.0), (187.0, 117.5), 7.0, 1.75, "fin", below=6.0, edge=True, boot=True))
    add(swept_trail("rudder", (213.0, 57.0), (222.0, 117.5), 10.0, 1.75, "fin", below=0.5, rudder=True))
    add(Box("dorsal", (-1, 50.5, 104), (1, 62, 154), "fin", origin=(0, 62, 104), rot=(-13.5, 0, 0)))
    add(Box("fin_cap", (-2.5, 116, 184), (2.5, 121.4, 225.2), "bullet"))
    add(Box("tailplane", (-58.5, 117.5, 201), (58.5, 120, 221), "wing", te=221, flap=7))
    a_tp = math.degrees(math.atan2(8.0, 55.0))
    boxes += pair(Box("tailplane_edge_right", (3.5, 117.75, 193), (3.5 + 55 / math.cos(math.radians(a_tp)), 119.75, 203.5),
                      "wing", origin=(3.5, 118.75, 193), rot=(0, -a_tp, 0), le=193, boot=2))
    add(Box("beacon", (-0.75, 121.4, 200), (0.75, 122.4, 203), "beacon", lamp="glow"))
    add(Box("beacon_belly", (-0.75, 14.25, 50), (0.75, 15.25, 53), "beacon", lamp="glow"))
    add(Box("tail_light", (-0.75, 51, 222), (0.75, 53, 222.6), "white_light", lamp="glow"))
    add(Box("antenna", (-0.25, 62, -120), (0.25, 66, -116), "antenna"))
    add(Box("antenna_aft", (-0.25, 62, 60), (0.25, 66, 64), "antenna"))

    # Landing gear, all of it in gear_* bones: the airframe is closed without them.
    nz = -172.8
    for b in wheel("gear_nose_wheel_right", 2.5, 3.5, nz, 3.5, 1.25, "gear_nose"):
        boxes += pair(b)
    add(Box("gear_nose_axle", (-1.25, 2.75, nz - 0.75), (1.25, 4.25, nz + 0.75), "gear", "gear_nose"))
    add(Box("gear_nose_leg", (-1.1, 4, nz - 1.1), (1.1, 19.5, nz + 1.1), "gear", "gear_nose", oleo=True))
    boxes += pair(Box("gear_nose_door_right", (4.4, 12.5, nz - 6.5), (4.7, 17.6, nz + 4.5), "gear", "gear_nose", door=True))
    for name, wx in (("gear_main_inner_right", 28.3), ("gear_main_outer_right", 37.3)):
        for b in wheel(name, wx, 7.0, 0.0, 7.0, 2.0, "gear_main", fat=True):
            boxes += pair(b)
    boxes += pair(Box("gear_main_axle_right", (30.3, 6, -1), (35.3, 8, 1), "gear", "gear_main"))
    boxes += pair(Box("gear_main_leg_right", (31.4, 7, -1.75), (34.2, 15.5, 1.75), "gear", "gear_main", oleo=True))
    boxes += pair(Box("gear_main_link_right", (31.9, 4.5, 1.75), (33.7, 6.5, 8), "gear", "gear_main"))

    painters = {
        "fus": rg_fus,
        "belly": belly_paint(30.0),
        "windscreen": ws_paint,
        "pillar": lambda c: Paint("body", 0.12 if c.face == "up" else 0.03),
        "wing": wing_paint,
        "tip": bz_tip,
        "fairing": rg_fairing,
        "nav": paint_nav,
        "landing": lambda c: LANDING,
        "nacelle": rg_nacelle,
        "spinner": rg_spinner,
        "prop": prop_paint,
        "pod": rg_pod,
        "fin": bz_fin,
        "bullet": bz_bullet,
        "beacon": lambda c: BEACON,
        "white_light": lambda c: NAV_WHITE,
        "antenna": lambda c: WING_LO,
        "gear": bz_gear,
        "wheel": paint_wheel,
    }
    return Model("plane_regional", boxes, painters, LIVERY, eye=(0.0, 43.5, -98.0))


MODELS = [build_regional, build_bizjet, build_light]
