"""Mainline trains: the ICE's end and middle cars, an electric locomotive and the freight
wagons, at the sizes `train_bodies.rs` gives them (`length_m`, `coach`, `cab`, `loco`,
`wagon_frame`, `built`). That file has +z ahead; here the front is at -z, so its z is our -z.
"""
import math

from kit import (
    AMBER, CHROME, CHROME_DARK, DRL, GLASS_DARK, GLASS_MID, GLOSS, HOUSING, HOUSING_DARK, LENS,
    TAIL_BRIGHT, TAIL_DARK, TRIM, UNDER, Box, Model, Paint, glass, hexc, mix, pair,
)

M = 16.0  # units to the metre

# ---------------------------------------------------------------- shared colours

RUBBER = hexc("#1d1d20")
RUBBER_FOLD = hexc("#2c2c31")
FRAME = hexc("#383c43")
FRAME_HI = hexc("#565c65")
FRAME_LO = hexc("#24272c")
AXLEBOX = hexc("#4a4f57")
AXLEBOX_HI = hexc("#6d737c")
SPRING_HI = hexc("#8d939c")
SPRING_LO = hexc("#2a2d32")
WHEEL_WEB = hexc("#3e4248")
WHEEL_DISC = hexc("#575c63")
WHEEL_RIM = hexc("#8a9098")
WHEEL_TREAD = hexc("#a4a9b0")
WHEEL_HUB = hexc("#6c727a")
STEEL_DARK = hexc("#4b5058")
STEEL_LIGHT = hexc("#8d939b")
INSULATOR = hexc("#7f8a7a")
INSULATOR_RIB = hexc("#5c6658")
COPPER = hexc("#8a5a3c")
CARBON = hexc("#2a2a2c")
GRILLE_DARK = hexc("#1b1c1f")
SIGN = hexc("#f2a53a")
SIGN_DARK = hexc("#6b4210")
YELLOW = hexc("#e7b52c")


def flo(v):
    return math.floor(v + 1e-6)


def zmirror(box, name):
    """The same box at the other end (z -> -z)."""
    return Box(
        name,
        (box.frm[0], box.frm[1], -box.to[2]),
        (box.to[0], box.to[1], -box.frm[2]),
        box.mat, box.bone,
        (box.origin[0], box.origin[1], -box.origin[2]),
        (-box.rot[0], -box.rot[1], box.rot[2]),
        box.lamp, **box.extra,
    )


def both_ends(boxes):
    """Boxes for the -z end and their mirror images at the +z end."""
    out = []
    for b in boxes:
        out.append(b)
        out.append(zmirror(b, b.name.replace("front", "rear") if "front" in b.name else b.name + "_r"))
    return out


def slab(name, w, base, top, t, mat, x0=None, ext=(0.0, 0.0), **extra):
    """A plate of thickness t leaned about x, its upper face running from (y, z) `base` to
    `top`, `ext` units longer at either end; w its half width (or x from x0 to w)."""
    (yb, zb), (yt, zt) = base, top
    dz, dy = zt - zb, yt - yb
    length = math.hypot(dz, dy)
    ang = math.degrees(math.atan2(dy, abs(dz)))
    x0 = -w if x0 is None else x0
    if dz >= 0:
        # rises towards +z: turn the far end up
        return Box(name, (x0, yb - t, zb - ext[0]), (w, yb, zb + length + ext[1]), mat,
                   origin=(0, yb, zb), rot=(-ang, 0, 0), **extra)
    return Box(name, (x0, yb - t, zb - length - ext[1]), (w, yb, zb + ext[0]), mat,
               origin=(0, yb, zb), rot=(ang, 0, 0), **extra)


# ---------------------------------------------------------------- bogies


def wheel_side(dz, dy, r):
    """The outer face of a rail wheel: tread edge, rim, web with a brake disc, hub."""
    q = math.hypot(dz, dy)
    if q < r * 0.22:
        return WHEEL_HUB
    if q < r * 0.32:
        return STEEL_DARK
    if q < r * 0.66:
        return WHEEL_DISC
    if q < r * 0.78:
        return WHEEL_WEB
    if q < r * 0.9:
        return WHEEL_RIM
    return WHEEL_TREAD


def paint_wheel(c):
    wz, wy, r = c.box.extra["axle"]
    if c.side:
        return wheel_side(c.z - wz, c.y - wy, r)
    if c.top or c.bottom or c.front or c.back:
        # tread and flange
        return WHEEL_TREAD if abs(c.x) > 11.2 else STEEL_DARK
    return WHEEL_WEB


def paint_frame(c):
    lo, hi = c.box.frm[1], c.box.to[1]
    if c.top:
        return FRAME_HI
    if c.bottom:
        return FRAME_LO
    y = c.loc[1]
    if y > hi - 1:
        return FRAME_HI
    if y < lo + 1:
        return FRAME_LO
    return FRAME


def paint_axlebox(c):
    if c.side:
        dz = c.z - c.box.extra["centre"][0]
        dy = c.y - c.box.extra["centre"][1]
        q = max(abs(dz), abs(dy))
        if q < 1.5:
            return AXLEBOX_HI
        if q < 2.5:
            return FRAME_LO
        if c.loc[1] > c.box.to[1] - 1:
            return AXLEBOX_HI
        return AXLEBOX
    return AXLEBOX_HI if c.top else AXLEBOX


def paint_spring(c):
    if c.top or c.bottom:
        return SPRING_LO
    return SPRING_HI if flo(c.loc[1] - c.box.frm[1]) % 2 == 0 else SPRING_LO


def paint_airspring(c):
    if c.side or c.front or c.back:
        r = flo(c.loc[1] - c.box.frm[1])
        return RUBBER_FOLD if r in (1, 3) else RUBBER
    return RUBBER


def paint_brake(c):
    if c.top:
        return STEEL_LIGHT
    return FRAME_LO if c.bottom else STEEL_DARK


BOGIE_PAINTERS = {
    "wheel": paint_wheel,
    "bframe": paint_frame,
    "axlebox": paint_axlebox,
    "spring": paint_spring,
    "airspring": paint_airspring,
    "brake": paint_brake,
}


def wheelset(tag, az, r=7.0, half=12.6):
    """An axle's two wheels as one stepped disc across the track."""
    axle = (az, r, r)
    return [
        Box(f"{tag}_a", (-half, 0, az - r * 0.5), (half, 2 * r, az + r * 0.5), "wheel", axle=axle),
        Box(f"{tag}_b", (-half + 0.25, r * 0.25, az - r * 0.82), (half - 0.25, 2 * r - r * 0.25, az + r * 0.82), "wheel", axle=axle),
        Box(f"{tag}_c", (-half + 0.5, r * 0.55, az - r), (half - 0.5, 2 * r - r * 0.55, az + r), "wheel", axle=axle),
    ]


def passenger_bogie(tag, zc, wb=40.0, r=7.0, top=18.4):
    """An outside-frame bogie: two wheelsets, axle boxes on coil springs, a side frame
    that dips between the axles, air springs up to the body and an eddy-current brake."""
    out = []
    for k, az in enumerate((zc - wb / 2, zc + wb / 2)):
        out += wheelset(f"{tag}_axle{k}", az, r)
        out += pair(Box(f"{tag}_axlebox{k}_right", (12.6, r - 3.5, az - 4), (17.5, r + 3, az + 4), "axlebox", centre=(az, r)))
        out += pair(Box(f"{tag}_spring{k}_right", (13.5, r + 3, az - 2.5), (16.0, r + 7, az + 2.5), "spring"))
        out += pair(Box(f"{tag}_frame_end{k}_right", (13.0, r + 7, az - 6), (16.5, r + 10.5, az + 6), "bframe"))
    out += pair(Box(f"{tag}_frame_mid_right", (13.25, r + 1, zc - wb / 2 + 5), (16.0, r + 8, zc + wb / 2 - 5), "bframe"))
    out += pair(Box(f"{tag}_air_right", (12.0, r + 8, zc - 4), (17.0, top, zc + 4), "airspring"))
    # a yaw damper from the frame to the body
    out += pair(Box(f"{tag}_damper_right", (16.25, r + 6, zc + 5), (17.5, r + 7.5, zc + 15), "brake"))
    return out


# ---------------------------------------------------------------- pantograph


def paint_insulator(c):
    if c.side or c.front or c.back:
        return INSULATOR if flo(c.loc[1] - c.box.frm[1]) % 2 == 0 else INSULATOR_RIB
    return INSULATOR


def paint_panto(c):
    if c.top:
        return STEEL_LIGHT
    if c.bottom:
        return STEEL_DARK
    return hexc("#767c84")


def paint_panto_head(c):
    if c.top:
        z0, z1 = c.box.frm[2], c.box.to[2]
        k = flo(c.loc[2] - z0)
        return CARBON if k in (1, 3) else STEEL_LIGHT
    return STEEL_DARK


PANTO_PAINTERS = {"insulator": paint_insulator, "panto": paint_panto, "panto_head": paint_panto_head}


def pantograph(tag, roof, pz, knee=-1):
    """A single-arm pantograph standing on four insulators at `pz`, its knee towards `knee`
    (-1 the front)."""
    s = knee
    out = []
    for dz in (-8.0, 8.0):
        out += pair(Box(f"{tag}_ins{int(dz > 0)}_right", (5.0, roof, pz + dz - 1), (7.0, roof + 3.5, pz + dz + 1), "insulator"))
    out.append(Box(f"{tag}_base", (-7.5, roof + 3.5, pz - 10), (7.5, roof + 5, pz + 10), "panto"))
    hinge = (roof + 5.0, pz - s * 7)
    kneep = (roof + 12.0, pz + s * 13)
    head = (roof + 17.0, pz - s * 2)
    out.append(slab(f"{tag}_lower_arm", 1.25, hinge, kneep, 1.5, "panto"))
    out.append(slab(f"{tag}_upper_arm", 0.75, (kneep[0] + 0.5, kneep[1]), (head[0] - 0.5, head[1]), 1.0, "panto"))
    out.append(slab(f"{tag}_rod", 0.5, (hinge[0] + 0.5, hinge[1] - s * 3), (kneep[0] - 1.5, kneep[1] - s * 1), 0.75, "panto", x0=-0.5))
    out.append(Box(f"{tag}_head", (-11.0, head[0] - 0.5, head[1] - 2.5), (11.0, head[0] + 0.5, head[1] + 2.5), "panto_head"))
    out += pair(Box(f"{tag}_horn_right", (11.0, head[0] - 2.0, head[1] - 2.0), (13.0, head[0], head[1] + 2.0), "panto"))
    return out


# ---------------------------------------------------------------- ICE

ICE_HW = 2.95 / 2 * M        # 23.6
ICE_F = 1.15 * M             # floor, 18.4
ICE_WALL = ICE_F + 40        # 58.4, where the roof begins to turn in
ICE_EAVE = ICE_WALL + 2      # 60.4
ICE_TOP = ICE_WALL + 4       # 62.4
ICE_EAVE_HW = ICE_HW - 2     # 21.6
ICE_ROOF_HW = ICE_HW - 4     # 19.6
ICE_SKIRT = 8.8
WIN_LO, WIN_HI = 16, 31      # window rows over the floor (2.15 m to 3.15 m)
STRIPE_ROWS = (12, 14)       # 1.9 m to 2.09 m
BAY = 30.0


def ice_row(r):
    """The ICE's flank at row r over the floor, away from windows and doors."""
    if r <= 0:
        return Paint("body", -0.24)
    if r == 1:
        return Paint("body", -0.1)
    if STRIPE_ROWS[0] <= r <= STRIPE_ROWS[1]:
        return Paint("stripe", {12: 0.1, 13: 0.0, 14: -0.12}[r])
    if r >= 38:
        return Paint("body", 0.1)
    return Paint("body", 0.0)


def ice_window(s, r, a, b, w0, w1):
    """A window from s = w0 to w1 (exclusive) in the band; None outside it."""
    if not (WIN_LO <= r <= WIN_HI and w0 <= s < w1):
        return None
    edge_s = s in (w0, w1 - 1)
    edge_r = r in (WIN_LO, WIN_HI)
    if edge_s and edge_r:
        return Paint("body", 0.0)
    if edge_s or edge_r:
        return GLOSS
    if (s in (w0 + 1, w1 - 2)) and (r in (WIN_LO + 1, WIN_HI - 1)):
        return GLOSS
    return glass(ICE_F + r + 0.5, a, b, top=ICE_F + WIN_HI, bottom=ICE_F + WIN_LO)


def ice_door(s, r, d0):
    """A plug door 16 wide from s = d0, in the door colour; None outside it."""
    k = s - d0
    if not (0 <= k < 16 and r <= 33):
        return None
    if k in (0, 15) or r == 33:
        return Paint("door", -0.5)
    if r == 0:
        return GLOSS
    if 4 <= k <= 11 and 18 <= r <= 29:
        if k in (4, 11) or r in (18, 29):
            return GLOSS
        return glass(ICE_F + r + 0.5, k, r, top=ICE_F + 29, bottom=ICE_F + 18)
    if STRIPE_ROWS[0] <= r <= STRIPE_ROWS[1]:
        return ice_row(r)
    if k == 12 and 15 <= r <= 17:
        return CHROME_DARK
    if r == 1:
        return Paint("door", -0.2)
    return Paint("door", 0.12 if r >= 31 else 0.0)


def ice_flank(c, kind):
    """The side of a body segment: `kind` says what is on it."""
    r = flo(c.loc[1] - ICE_F)
    z0, z1 = c.box.frm[2], c.box.to[2]
    if kind == "bay":
        s = flo(c.loc[2] - z0)
        return ice_window(s, r, s, r, 3, 27) or ice_row(r)
    if kind == "door":
        s = flo(c.loc[2] - z0)
        return ice_door(s, r, 0) or ice_row(r)
    if kind == "end":
        # measured from the carriage end inwards, so both ends match
        s = flo(z1 - c.loc[2]) if c.loc[2] > 0 else flo(c.loc[2] - z0)
        out = ice_door(s, r, 15) if c.box.extra.get("door") else None
        if out:
            return out
        if s in (0, 14) and 2 <= r <= 37:
            return Paint("body", -0.14)
        if 3 <= s <= 10:
            # a small frosted window by the vestibule
            w = ice_window(s, r, 0, 0, 3, 11) if r >= 20 else None
            if w is not None:
                return Paint("body", -0.3) if isinstance(w, tuple) and w == GLOSS else (hexc("#7c8b99") if isinstance(w, tuple) else w)
        return ice_row(r)
    if kind == "cab":
        # behind the nose: the driver's side window runs on from it, then the cab wall
        s = flo(c.loc[2] - z0)
        if s < 9:
            out = ice_window(s + 12, r, s + 12, r, -100, 21)
            if out is not None:
                return out
        if s == 12 and 2 <= r <= 37:
            return Paint("body", -0.14)
        return ice_row(r)
    return ice_row(r)


def ice_segment_painter(kind):
    def paint(c):
        if c.bottom:
            return UNDER
        if c.side:
            return ice_flank(c, kind)
        if c.top:
            return Paint("body", 0.12)
        if kind == "cab":
            return Paint("body", -0.2)
        # an end face: the carriage end around the gangway
        return Paint("body", -0.06)
    return paint


def paint_ice_eave(c):
    if c.side:
        return Paint("body", 0.14)
    if c.top:
        return Paint("body", 0.2)
    return Paint("body", 0.0)


def paint_ice_roof(c):
    if c.top:
        ax = abs(c.x)
        if ax > ICE_ROOF_HW - 1:
            return Paint("roof", 0.25)
        s = flo(c.loc[2] - c.box.frm[2])
        if s == 0:
            return Paint("roof", -0.12)
        if flo(ax) == 9:
            return Paint("roof", -0.06)
        return Paint("roof", 0.08)
    if c.side:
        return Paint("roof", 0.2)
    return Paint("roof", 0.0)


def paint_ice_skirt(c):
    if c.side:
        r = flo(c.loc[1] - c.box.frm[1])
        if r == 0:
            return Paint("body", -0.45)
        if r == 1:
            return Paint("body", -0.28)
        s = flo(c.loc[2] - c.box.frm[2])
        if s == 0:
            return Paint("body", -0.3)
        return Paint("body", -0.12)
    if c.bottom:
        return UNDER
    return Paint("body", -0.3)


def paint_bellows(c):
    if c.side or c.top:
        return RUBBER_FOLD if flo(c.z) % 2 == 0 else RUBBER
    # the end the next carriage's bellows meet: a folded rim round the gangway's frame
    e = min(c.x - c.box.frm[0], c.box.to[0] - c.x, c.y - c.box.frm[1], c.box.to[1] - c.y)
    if e < 4:
        return RUBBER_FOLD if flo(e) % 2 == 0 else RUBBER
    if e < 5:
        return STEEL_DARK
    return hexc("#26282c")


def paint_endcap(c):
    if c.front or c.back:
        ax, y = abs(c.x), c.y
        if ax > ICE_EAVE_HW - 1.5 or y < c.box.frm[1] + 1.5:
            return Paint("body", -0.18)
        return Paint("body", -0.08)
    return Paint("body", -0.14)


def paint_ac(c):
    if c.top:
        ax = abs(c.x)
        s = flo(c.loc[2] - c.box.frm[2])
        if 2 <= s <= c.box.to[2] - c.box.frm[2] - 3 and ax < 9:
            if flo(ax) in (2, 5, 8) or s % 3 == 0:
                return Paint("roof", -0.45)
            return GRILLE_DARK
        return Paint("roof", 0.12)
    return Paint("roof", -0.08)


def ice_body_segment(name, z0, z1, kind, **extra):
    mat = f"ice_flank_{kind}"
    return [
        Box(name, (-ICE_HW, ICE_F, z0), (ICE_HW, ICE_WALL, z1), mat, **extra),
        Box(name + "_eave", (-ICE_EAVE_HW, ICE_WALL, z0), (ICE_EAVE_HW, ICE_EAVE, z1), "ice_eave"),
        Box(name + "_roof", (-ICE_ROOF_HW, ICE_EAVE, z0), (ICE_ROOF_HW, ICE_TOP, z1), "ice_roof"),
    ]


def ice_painters():
    p = {
        "ice_flank_bay": ice_segment_painter("bay"),
        "ice_flank_door": ice_segment_painter("door"),
        "ice_flank_end": ice_segment_painter("end"),
        "ice_flank_cab": ice_segment_painter("cab"),
        "ice_eave": paint_ice_eave,
        "ice_roof": paint_ice_roof,
        "ice_skirt": paint_ice_skirt,
        "bellows": paint_bellows,
        "endcap": paint_endcap,
        "ac": paint_ac,
    }
    p.update(BOGIE_PAINTERS)
    p.update(PANTO_PAINTERS)
    return p


def ice_gangway_end(tag, zend, sign):
    """The carriage end at zend (sign +1 at +z): its rounded end and the bellows."""
    # The bellows reach 0.2 m past the carriage's length, half the 0.4 m coupling gap, so
    # neighbours' bellows meet.
    a, b = (zend, zend + 2.4 * sign)
    c0, c1 = (zend + 2.4 * sign, zend + (2.4 + 3.2) * sign)
    return [
        Box(f"{tag}_endcap", (-ICE_EAVE_HW, ICE_F + 1, min(a, b)), (ICE_EAVE_HW, ICE_WALL - 1, max(a, b)), "endcap"),
        Box(f"{tag}_bellows", (-15.6, ICE_F + 2.4, min(c0, c1)), (15.6, ICE_WALL - 1.4, max(c0, c1)), "bellows"),
    ]


def skirt_panels(tag, z0, z1, n, low=ICE_SKIRT):
    """The skirt between the bogies as n like panels, so they share their texture."""
    step = (z1 - z0) / n
    return [Box(f"{tag}{k}", (-ICE_HW + 1, low, z0 + step * k), (ICE_HW - 1, ICE_F, z0 + step * (k + 1)), "ice_skirt")
            for k in range(n)]


ICE_INTERCITY = {
    "body": "minecraft:white_concrete",
    "lower": "minecraft:white_concrete",
    "stripe": "minecraft:red_concrete",
    "door": "minecraft:light_gray_concrete",
    "roof": "minecraft:light_gray_concrete",
}


def build_ice_mid():
    half = 24.8 / 2 * M  # 198.4
    zb = half - 2.4      # body ends 0.15 m in, as train_bodies.rs has it
    boxes = []
    # 11 window bays between the doors, a door and the vestibule at either end.
    for k in range(11):
        z0 = -165 + BAY * k
        boxes += ice_body_segment(f"bay{k}", z0, z0 + BAY, "bay")
    boxes += ice_body_segment("end_front", -zb, -165, "end", door=True)
    boxes += ice_body_segment("end_rear", 165, zb, "end", door=True)
    boxes += ice_gangway_end("front", -zb, -1)
    boxes += ice_gangway_end("rear", zb, 1)
    # Skirts between and beyond the bogies (at +-150.4).
    bz = half - 3.0 * M
    boxes += skirt_panels("skirt_mid", -bz + 26, bz - 26, 8)
    boxes.append(Box("skirt_front", (-ICE_HW + 1, ICE_SKIRT + 2, -zb), (ICE_HW - 1, ICE_F, -bz - 26), "ice_skirt"))
    boxes.append(Box("skirt_rear", (-ICE_HW + 1, ICE_SKIRT + 2, bz + 26), (ICE_HW - 1, ICE_F, zb), "ice_skirt"))
    boxes += passenger_bogie("bogie_front", -bz)
    boxes += passenger_bogie("bogie_rear", bz)
    # The air-conditioning unit (train_bodies.rs: half/2 - 0.5 m from the middle).
    az = -(half / 2 - 0.5 * M)
    boxes.append(Box("ac", (-12, ICE_TOP, az - 20.8), (12, ICE_TOP + 3.5, az + 20.8), "ac"))
    return Model(
        "train_ice_mid", boxes, ice_painters(), ICE_INTERCITY,
        eye=(0.0, 2.12 * M, 0.0),
        notes="ICE middle car, 24.8 m; doors at +-10.8 m",
    )


# ---------------------------------------------------------------- ICE nose

def prof_y(prof, d):
    for (d0, y0), (d1, y1) in zip(prof, prof[1:]):
        if d <= d1:
            return y0 + (y1 - y0) * max(0.0, d - d0) / (d1 - d0)
    return prof[-1][1]


def prof_d(prof, y):
    for (d0, y0), (d1, y1) in zip(prof, prof[1:]):
        if y <= y1:
            return d0 + (d1 - d0) * max(0.0, y - y0) / (y1 - y0)
    return prof[-1][0]


def nose_band(tag, w, top, z_of, joint, t=3.2, chin=None, fill_to=None, steps=None, step_w=None, h=3.0):
    """One width band of a streamlined nose, w its half width.

    `top` is the band's upper line, (d from the tip, y), drawn as plates t thick leaned
    along it. A plate's front end leans forward, so it is buried in a `chin` = (thickness,
    (d, y) where its lower edge ends): a plate falling away backwards whose upper front edge
    is the top line's first point. The tip is then a bullet: the chin's foremost edge, its
    end leaning back, the top plate leaning back further. Fill boxes close the wedge between
    chin and plate up to d = `fill_to`, where the next band outside takes over. The
    outermost band instead stands on level `steps` up to that height, step_w wide."""
    out = []
    n = len(top) - 1
    angs = [math.atan2(top[i + 1][1] - top[i][1], top[i + 1][0] - top[i][0]) for i in range(n)]
    for i in range(n):
        (d0, y0), (d1, y1) = top[i], top[i + 1]
        pre = t if i > 0 and angs[i] > angs[i - 1] else 0.0
        post = t if i < n - 1 and angs[i + 1] > angs[i] else 0.0
        # Neighbouring plates overlap below their joint: the windscreen plate stands 0.25
        # proud of the others so their flanks never lie in one plane.
        wi = w if i == 1 else w - 0.25
        out.append(slab(f"{tag}_plate{i}", wi, (y0, z_of(d0)), (y1, z_of(d1)), t, "nose_plate",
                        ext=(pre, post), seg=i, band=tag, line=((d0 - pre, y0), (d1, y1))))
    a0 = angs[0]
    r = top[0]
    plate_top = lambda d: prof_y(top, d)
    plate_bottom = lambda d: r[1] - t / math.cos(a0) + (d - r[0]) * math.tan(a0)
    if chin:
        tc, end = chin
        b = math.radians(30)
        for _ in range(30):
            p = (r[0] - tc * math.sin(b), r[1] - tc * math.cos(b))
            b = math.atan2(p[1] - end[1], end[0] - p[0])
        beta = math.degrees(b)
        assert p[0] >= 0 and beta <= 45, (tag, p, beta)
        length = math.hypot(end[0] - p[0], end[1] - p[1])
        out.append(Box(f"{tag}_chin", (-w, p[1], z_of(p[0])), (w, p[1] + tc, z_of(p[0]) + length), "nose_chin",
                       origin=(0, p[1], z_of(p[0])), rot=(beta, 0, 0), band=tag))
        chin_top = lambda d: r[1] - (d - r[0]) * math.tan(b)
        chin_low = lambda d: p[1] - (d - p[0]) * math.tan(b)
        if fill_to:
            # where chin and plate part, then boxes that each reach the gap's size a little
            # further back without standing out of the band at their own front
            d = r[0] + (r[1] - plate_bottom(r[0])) / (math.tan(a0) + math.tan(b))
            k = 0
            while d < fill_to - 0.5:
                nxt = min(fill_to, d + 3.0)
                lo = max(chin_top(nxt) - 0.2, chin_low(d) + 0.3)
                hi = min(plate_bottom(nxt) + 0.2, plate_top(d) - 0.3)
                out.append(Box(f"{tag}_fill{k}", (-w + 0.5, lo, z_of(d)), (w - 0.5, hi, z_of(fill_to)), "nose_core", band=tag))
                d, k = nxt, k + 1
    if steps:
        y = ICE_F
        k = 0
        while y < steps - 1e-6:
            up = min(y + h, steps)
            if y < r[1] - 1e-6 < up:
                up = r[1]
            d_front = r[0] if up <= r[1] else prof_d(top, up)
            dz = joint - z_of(d_front)
            zf = joint - math.floor(dz + 1e-6)
            if zf > joint - 1:
                break
            out.append(Box(f"{tag}_step{k}", (-step_w, y, zf), (step_w, up, joint), "nose_core", band=tag))
            y, k = up, k + 1
    return out


# The nose's bands, innermost first: half width, upper line (d from the tip, y), plate
# thickness, chin, where the fill ends. Each outer band starts further back and lower, and
# meets the body's flank, eave or roof at the joint.
NOSE_LEN = 3.6 * M
NOSE_BANDS = [
    ("nose_C", 10.0, [(3.5, 25.4), (13.0, 33.4), (42.0, 54.2), (NOSE_LEN, ICE_TOP)], 6.0, (6.0, (19.0, 11.4)), 11.0),
    ("nose_B", ICE_ROOF_HW - 0.25, [(8.5, 24.4), (18.0, 33.4), (42.0, 52.4), (NOSE_LEN, ICE_TOP)], 5.0, (5.0, (19.0, 12.4)), 16.0),
    ("nose_A2", ICE_EAVE_HW - 0.25, [(14.0, 24.9), (24.0, 34.4), (42.0, 49.9), (NOSE_LEN, ICE_EAVE)], 4.0, (4.0, (20.0, 15.4)), 21.0),
    ("nose_A", ICE_HW - 0.3, [(19.5, 26.4), (30.0, 36.4), (47.0, 50.4), (NOSE_LEN, ICE_WALL)], 3.2, None, None),
]
# The ends of the dark band round the cab on the nose's flanks: lines square across the
# windscreen plates through these points.
WRAP_CUTS = ((24.0, 34.4), (42.0, 49.9))
WRAP_ANGLE = math.radians(40)


def cut_along(a, b, p):
    """How far along the line a-b a line through p square to WRAP_ANGLE crosses it."""
    (d0, y0), (d1, y1) = a, b
    length = math.hypot(d1 - d0, y1 - y0)
    ex, ey = (d1 - d0) / length, (y1 - y0) / length
    fx, fy = -math.sin(WRAP_ANGLE), math.cos(WRAP_ANGLE)
    # a + u e = p + v f
    det = ex * (-fy) - ey * (-fx)
    rx, ry = p[0] - d0, p[1] - y0
    return (rx * (-fy) - ry * (-fx)) / det


# Where the windscreen glass begins on each band (y), so it narrows towards its foot.
GLASS_FROM = {"nose_C": 35.0, "nose_B": 38.0, "nose_A2": 42.0}


def paint_nose_plate(c):
    band, seg = c.box.extra["band"], c.box.extra["seg"]
    s = c.loc[2] - c.box.frm[2]           # along the plate from its lower end
    length = c.box.to[2] - c.box.frm[2]
    glass_from = GLASS_FROM.get(band)
    in_glass = glass_from is not None and seg == 1
    if c.face == "up":
        if in_glass and c.y >= glass_from:
            if s > length - 1.0 or c.y < glass_from + 1.0:
                return GLOSS
            if band == "nose_A2" and abs(c.x) > c.box.to[0] - 0.75:
                return GLOSS
            return glass(c.y, math.floor(c.x), math.floor(s), top=54, bottom=33)
        if seg == 2:
            return Paint("roof", 0.1) if band in ("nose_C", "nose_B") and s > 4 else Paint("body", 0.14)
        return Paint("body", 0.1)
    if c.face == "down":
        return UNDER
    if c.side:
        if in_glass:
            # The dark band round the cab: from one cut to the other, both square across
            # the plates and lined up across the bands, so its ends run straight.
            (d0, y0), (d1, y1) = c.box.extra["line"]
            sa, sb = (cut_along((d0, y0), (d1, y1), p) for p in WRAP_CUTS)
            if sa <= s < sb:
                return GLOSS
        return Paint("body", 0.0)
    # the plate's ends lean back from the tip
    return Paint("body", 0.02)


def paint_nose_core(c):
    """Cores and steps of the nose: the ICE's flank carried forward, or its front."""
    if c.bottom:
        return UNDER
    if c.side:
        r = flo(c.loc[1] - ICE_F)
        z = c.loc[2]
        d = z - (-25.8 / 2 * M)
        if r < 0:
            return Paint("body", -0.2 if r == -1 else -0.08)
        # the driver's side window, cut level at the plate above it
        if WIN_LO <= r <= WIN_HI and d >= 36:
            out = ice_window(flo(d - 36), r, flo(d), r, 0, 40)
            if out is not None:
                return out
        return ice_row(r)
    if c.front:
        y = c.loc[1]
        return Paint("body", -0.18 if y < ICE_F - 6 else -0.08)
    return Paint("body", 0.0)


CHIN_SHADE = {"nose_A2": -0.1, "nose_B": -0.2, "nose_C": -0.3}


def paint_nose_chin(c):
    """The nose's underside sweeping up to the tip, darker the further under it lies."""
    k = CHIN_SHADE[c.box.extra["band"]]
    if c.face == "down":
        s = c.loc[2] - c.box.frm[2]
        return Paint("body", k - 0.25) if s < 2 else hexc("#3a3d43")
    if c.face == "north":
        return Paint("body", k + 0.04)
    if c.side:
        # a dark lip along the lower edge
        if c.loc[1] < c.box.frm[1] + 1.0:
            return Paint("body", k - 0.25)
        return Paint("body", k)
    return Paint("body", k - 0.1)


def paint_ice_lamp(c):
    if c.face == "up":
        s = flo(c.loc[2] - c.box.frm[2])
        n = round(c.box.to[2] - c.box.frm[2])
        i = flo(c.loc[0] - c.box.frm[0])
        w = round(c.box.to[0] - c.box.frm[0])
        if s in (0, n - 1) or i in (0, w - 1):
            return HOUSING_DARK
        if c.box.extra.get("marker"):
            return DRL
        if i == w - 3 and 2 <= s <= n - 3:
            return LENS
        return DRL
    return HOUSING_DARK


def paint_ice_tail(c):
    if c.front or c.face == "up":
        return TAIL_BRIGHT
    return TAIL_DARK


def ice_nose(z_tip):
    """The streamlined nose from z_tip (the front) back 3.6 m."""
    z_of = lambda d: z_tip + d
    joint = z_of(NOSE_LEN)
    boxes = []
    for tag, w, top, t, chin, fill_to in NOSE_BANDS:
        if tag == "nose_A":
            boxes += nose_band(tag, w, top, z_of, joint, t=t, steps=ICE_WALL, step_w=ICE_HW - 0.8)
        else:
            boxes += nose_band(tag, w, top, z_of, joint, t=t, chin=chin, fill_to=fill_to)
    # A strip down the joint where the flank meets the nose's narrower steps.
    boxes += pair(Box("nose_crease_right", (ICE_HW - 0.8, ICE_F, joint - 0.5), (ICE_HW, ICE_WALL, joint), "nose_crease"))
    # The skirt in front of the bogie the chins run into.
    boxes.append(Box("nose_skirt", (-ICE_HW + 1, ICE_SKIRT + 1, z_of(17.5)), (ICE_HW - 1, ICE_F, z_of(20.5)), "ice_skirt"))
    # Head and tail lamps on the B band's bonnet, a marker lamp over the windscreen.
    (d0, y0), (d1, y1) = NOSE_BANDS[1][2][:2]
    a = math.atan2(y1 - y0, d1 - d0)
    on_b = lambda dd: (y0 + dd * math.sin(a) + 0.5, z_of(d0 + dd * math.cos(a)))
    boxes += pair(slab("nose_head_right", NOSE_BANDS[1][1] - 0.75, on_b(3.0), on_b(9.0), 1.0, "ice_lamp", x0=13.0, lamp="head"))
    boxes += pair(slab("nose_tail_right", 12.5, on_b(4.0), on_b(7.0), 1.0, "ice_tail", x0=10.5, lamp="glow"))
    (d0, y0), (d1, y1) = NOSE_BANDS[0][2][2:4]
    a = math.atan2(y1 - y0, d1 - d0)
    on_c = lambda dd: (y0 + dd * math.sin(a) + 0.5, z_of(d0 + dd * math.cos(a)))
    boxes.append(slab("nose_marker", 2.5, on_c(1.0), on_c(4.0), 0.75, "ice_lamp", lamp="head", marker=True))
    return boxes


def build_ice_cab():
    half = 25.8 / 2 * M  # 206.4
    zb = half - 2.4      # 204, the gangway end
    tip = -half
    joint = tip + NOSE_LEN  # -148.8
    boxes = ice_nose(tip)
    boxes += ice_body_segment("cab_section", joint, -128, "cab")
    for k in range(4):
        z0 = -128 + BAY * k
        boxes += ice_body_segment(f"bay{k}", z0, z0 + BAY, "bay")
    boxes += ice_body_segment("door", -8, 8, "door")
    for k in range(6):
        z0 = 8 + BAY * k
        boxes += ice_body_segment(f"bay{k + 4}", z0, z0 + BAY, "bay")
    boxes += ice_body_segment("end_rear", 188, zb, "end")
    boxes += ice_gangway_end("rear", zb, 1)
    bz = half - 3.0 * M  # bogies at +-158.4
    boxes += skirt_panels("skirt_mid", -bz + 26, bz - 26, 8)
    boxes.append(Box("skirt_rear", (-ICE_HW + 1, ICE_SKIRT + 2, bz + 26), (ICE_HW - 1, ICE_F, zb), "ice_skirt"))
    boxes += passenger_bogie("bogie_front", -bz)
    boxes += passenger_bogie("bogie_rear", bz)
    # Pantograph 6 m from the gangway end (train_bodies.rs: -half + 6 with the cab ahead),
    # the air conditioning half way to the cab.
    boxes += pantograph("panto", ICE_TOP, half - 6.0 * M, knee=-1)
    az = -(half / 2 - 0.5 * M)
    boxes.append(Box("ac", (-12, ICE_TOP, az - 20.8), (12, ICE_TOP + 3.5, az + 20.8), "ac"))
    painters = ice_painters()
    painters.update({
        "nose_plate": paint_nose_plate,
        "nose_core": paint_nose_core,
        "nose_chin": paint_nose_chin,
        "nose_crease": lambda c: Paint("body", -0.2) if c.front else ice_row(flo(c.y - ICE_F)) if c.side else Paint("body", 0.1),
        "ice_lamp": paint_ice_lamp,
        "ice_tail": paint_ice_tail,
    })
    return Model(
        "train_ice_cab", boxes, painters, ICE_INTERCITY,
        eye=(0.0, 2.12 * M, 0.0),
        notes="ICE end car, 25.8 m with a 3.6 m nose at -z; door in the middle",
    )


# ---------------------------------------------------------------- locomotive

LOCO_HALF = 19.5 / 2 * M     # 156
LOCO_END = LOCO_HALF - 2.4   # 153.6, the body ends 0.15 m in
LOCO_HW = 3.0 / 2 * M        # 24
LOCO_F = 1.15 * M            # 18.4
LOCO_WALL = LOCO_F + 41      # 59.4
LOCO_EAVE = LOCO_WALL + 3    # 62.4
LOCO_TOP = 4.1 * M           # 65.6
LOCO_EAVE_HW = LOCO_HW - 2
LOCO_ROOF_HW = LOCO_HW - 4
LOCO_BAY = 27.4
LOCO_MID = 4 * LOCO_BAY      # the machine room runs +-109.6
MASK_LEAN = 17.0             # the windscreen leans back this far from upright
MASK_FOOT = 38.0
LOUVRE = hexc("#3c4048")
LOUVRE_HI = hexc("#5d636c")
STEP_TREAD = hexc("#7b818a")


def loco_row(r):
    """The locomotive's flank at row r over the floor."""
    if r <= 5:
        return Paint("lower", -0.18 if r == 0 else (0.12 if r == 5 else 0.0))
    if r == 6:
        return Paint("body", -0.2)
    if 9 <= r <= 11:
        return Paint("stripe", {9: 0.08, 10: 0.0, 11: -0.1}[r])
    if r >= 39:
        return Paint("body", 0.12)
    return Paint("body", 0.0)


def loco_grille(s, r, s0, s1, r0=20, r1=36):
    """A louvred grille from s0 to s1 (exclusive), rows r0 to r1; None outside it."""
    if not (s0 <= s < s1 and r0 <= r <= r1):
        return None
    if s in (s0, s1 - 1) or r in (r0, r1):
        return GLOSS
    return LOUVRE_HI if (r - r0) % 2 == 1 else GRILLE_DARK


def loco_flank(c, kind):
    r = flo(c.loc[1] - LOCO_F)
    if kind == "bay":
        s = flo(c.loc[2] - c.box.frm[2])
        if s == 0 and 6 <= r <= 38:
            return Paint("body", -0.18)
        return loco_grille(s, r, 3, 25) or loco_row(r)
    if kind == "plain":
        s = flo(c.loc[2] - c.box.frm[2])
        if s == 0 and 6 <= r <= 38:
            return Paint("body", -0.18)
        if 3 <= s <= 24 and r == 37:
            return Paint("body", -0.1)
        return loco_row(r)
    # the cab end, measured from the body's end inwards
    s = flo(LOCO_END - abs(c.loc[2]))
    # the cab door, with a window, grab rails either side
    if 16 <= s < 30 and 1 <= r <= 37:
        k = s - 16
        if k in (0, 13) or r in (1, 37):
            return Paint("door", -0.5)
        if 3 <= k <= 10 and 22 <= r <= 33:
            if k in (3, 10) or r in (22, 33):
                return GLOSS
            return glass(LOCO_F + r + 0.5, k, r, top=LOCO_F + 33, bottom=LOCO_F + 22)
        if k == 11 and 18 <= r <= 20:
            return CHROME_DARK
        if 9 <= r <= 11:
            return loco_row(r)
        return Paint("door", 0.1 if r >= 35 else 0.0)
    if s in (14, 31) and 4 <= r <= 34:
        return CHROME_DARK
    # a small window ahead of the door
    if 10 <= s <= 13 and 22 <= r <= 33:
        if s in (10, 13) or r in (22, 33):
            return GLOSS
        return glass(LOCO_F + r + 0.5, s, r, top=LOCO_F + 33, bottom=LOCO_F + 22)
    if s == 33 and 6 <= r <= 38:
        return Paint("body", -0.18)
    return loco_grille(s, r, 36, 44, 20, 30) or loco_row(r)


def loco_segment_painter(kind):
    def paint(c):
        if c.bottom:
            return UNDER
        if c.side:
            return loco_flank(c, kind)
        if c.top:
            return Paint("body", 0.12)
        return Paint("body", -0.1)
    return paint


def paint_loco_eave(c):
    if c.side:
        return Paint("body", 0.16)
    if c.top:
        return Paint("roof", 0.1)
    return Paint("body", 0.0)


def paint_loco_roof(c):
    if c.top:
        s = flo(c.loc[2] - c.box.frm[2])
        ax = abs(c.x)
        if ax > LOCO_ROOF_HW - 1:
            return Paint("roof", 0.2)
        if s == 0:
            return Paint("roof", -0.15)
        if flo(ax) in (7, 8):
            return Paint("roof", -0.08)
        return Paint("roof", 0.04)
    if c.side:
        return Paint("roof", 0.16)
    return Paint("roof", -0.05)


def loco_segment(name, z0, z1, kind):
    return [
        Box(name, (-LOCO_HW, LOCO_F, z0), (LOCO_HW, LOCO_WALL, z1), f"loco_flank_{kind}"),
        Box(name + "_eave", (-LOCO_EAVE_HW, LOCO_WALL, z0), (LOCO_EAVE_HW, LOCO_EAVE, z1), "loco_eave"),
        Box(name + "_roof", (-LOCO_ROOF_HW, LOCO_EAVE, z0), (LOCO_ROOF_HW, LOCO_TOP, z1), "loco_roof"),
    ]


def outward(c):
    """Whether a face looks out of the end its box is at."""
    mid = (c.box.frm[2] + c.box.to[2]) / 2
    return (c.n[2] < -0.5 and mid < 0) or (c.n[2] > 0.5 and mid > 0)


def paint_loco_nose(c):
    """The cab's lower front: lamps sit on it, the stripe runs round it."""
    if c.bottom:
        return UNDER
    r = flo(c.loc[1] - LOCO_F)
    if outward(c):
        ax = abs(c.x)
        if r == 19:
            return Paint("body", 0.14)
        if ax > LOCO_HW - 1:
            return Paint("body", -0.1)
        return loco_row(r)
    if c.side:
        return loco_row(r)
    if c.top:
        return Paint("body", 0.14)
    return Paint("body", 0.0)


def paint_loco_mask(c):
    """The leaned upper front: windscreen in a black surround, pillars in the body colour."""
    ly = c.loc[1] - MASK_FOOT
    r = flo(ly)
    if outward(c):
        ax = abs(c.x)
        ix = flo(ax)
        if ax < 20.5 and 1 <= r <= 21:
            if ix == 20 or r in (1, 21):
                return GLOSS
            if r == 20 and ax < 4:
                return SIGN_DARK
            if 2 <= r <= 18 and ax < 19:
                if r == 2 and ix in (3, 4, 5, 6, 7) and c.x > 0:
                    return GLOSS  # wiper at rest
                return glass(c.y, math.floor(c.x), r, top=MASK_FOOT + 19, bottom=MASK_FOOT + 2)
            return GLOSS
        if r == 0:
            return Paint("body", 0.1)
        return Paint("body", -0.04 if ax > LOCO_HW - 1 else 0.04)
    if c.side:
        return Paint("body", 0.0)
    if c.top or c.face == "up":
        return Paint("body", 0.12)
    return Paint("body", -0.1)


def paint_loco_cabroof(c):
    if c.face == "up":
        return Paint("roof", 0.12) if c.box.extra.get("roofband") else Paint("body", 0.16)
    if c.side:
        return Paint("body", 0.12)
    return Paint("body", 0.0)


def paint_beam(c):
    if c.bottom:
        return UNDER
    if outward(c):
        r = flo(c.loc[1] - c.box.frm[1])
        n = round(c.box.to[1] - c.box.frm[1])
        if r == n - 1:
            return Paint("lower", 0.15)
        # yellow and black warning bars at the corners
        ax = abs(c.x)
        if ax > 18:
            return YELLOW if flo(ax + c.y) % 4 < 2 else GLOSS
        return Paint("lower", -0.1)
    return Paint("lower", 0.0 if c.top else -0.15)


def paint_buffer(c):
    if outward(c):
        return CHROME if c.box.extra.get("head") else STEEL_LIGHT
    if c.top:
        return STEEL_LIGHT
    return STEEL_DARK


def paint_plough(c):
    if outward(c):
        r = flo(c.loc[1] - c.box.frm[1])
        return GLOSS if r == 0 else hexc("#33363c")
    return GLOSS


def paint_loco_lamp(c):
    if outward(c):
        i = flo(c.loc[0] - c.box.frm[0])
        j = flo(c.loc[1] - c.box.frm[1])
        w = round(c.box.to[0] - c.box.frm[0])
        h = round(c.box.to[1] - c.box.frm[1])
        if i in (0, w - 1) or j in (0, h - 1):
            return HOUSING_DARK
        return LENS if (i == 1 and j == 1) else DRL
    return HOUSING_DARK


def paint_loco_tail(c):
    if outward(c):
        return TAIL_BRIGHT
    return TAIL_DARK


def loco_cab(tag):
    """The cab at the -z end; the other end is its mirror image."""
    e = -LOCO_END
    z = lambda d: e + d
    out = [
        # lower front, standing a little proud of the flanks like the mask above it
        Box(f"{tag}_nose", (-LOCO_HW - 0.5, LOCO_F, z(0)), (LOCO_HW + 0.5, MASK_FOOT, z(14)), "loco_nose"),
        # the mask: windscreen and pillars leaned back
        Box(f"{tag}_mask", (-LOCO_HW - 0.25, MASK_FOOT, z(1)), (LOCO_HW + 0.25, MASK_FOOT + 22.5, z(15)), "loco_mask",
            origin=(0, MASK_FOOT, z(1)), rot=(MASK_LEAN, 0, 0)),
    ]
    top_d = 1 + 22.5 * math.sin(math.radians(MASK_LEAN))
    top_y = MASK_FOOT + 22.5 * math.cos(math.radians(MASK_LEAN))
    out.append(slab(f"{tag}_cab_eave", LOCO_EAVE_HW - 0.25, (top_y, z(top_d)), (LOCO_EAVE, z(top_d + 4.5)), 3.0, "loco_cabroof"))
    out.append(slab(f"{tag}_cab_roof", LOCO_ROOF_HW - 0.25, (top_y + 1.4, z(top_d + 0.4)), (LOCO_TOP, z(top_d + 8.4)), 3.0, "loco_cabroof", roofband=True))
    # lamps: two head lamps and two tail lamps low, one head lamp over the windscreen
    out += pair(Box(f"{tag}_head_right", (13.5, 23.4, z(-0.25)), (19.5, 27.4, z(0)), "loco_lamp", lamp="head"))
    out += pair(Box(f"{tag}_tail_right", (9.5, 23.9, z(-0.25)), (12.5, 26.9, z(0)), "loco_tail", lamp="glow"))
    out.append(Box(f"{tag}_top_lamp", (-2.5, MASK_FOOT + 21.6, z(7.3)), (2.5, MASK_FOOT + 24.6, z(7.8)), "loco_lamp", lamp="head"))
    # buffer beam, buffers 0.2 m past the length, coupling, obstacle deflector
    out.append(Box(f"{tag}_beam", (-23.0, 10.4, z(-1.6)), (23.0, LOCO_F, z(10)), "beam"))
    out += pair(Box(f"{tag}_buffer_right", (12.0, 15.0, z(-4.6)), (16.0, 19.0, z(-1.6)), "buffer"))
    out += pair(Box(f"{tag}_buffer_head_right", (10.5, 13.5, -LOCO_HALF - 3.2), (17.5, 20.5, z(-4.6)), "buffer", head=True))
    out.append(Box(f"{tag}_coupler", (-1.5, 14.5, z(-4.0)), (1.5, 17.5, z(-1.6)), "buffer"))
    out.append(Box(f"{tag}_plough", (-20.0, 3.5, z(-1.2)), (20.0, 10.4, z(1.0)), "plough"))
    # footsteps under the cab door
    out += pair(Box(f"{tag}_footstep_right", (21.0, 8.0, z(17)), (23.6, LOCO_F, z(29)), "footstep"))
    return out


def paint_footstep(c):
    if c.side:
        r = flo(c.loc[1] - c.box.frm[1])
        return STEP_TREAD if r in (0, 5) else (GLOSS if r in (1, 6) else FRAME)
    if c.top:
        return STEP_TREAD
    return FRAME


def loco_bogie(tag, zc, wb=42.0, r=9.0, top=LOCO_F):
    """A locomotive's two-axle bogie: a deep side frame with coil springs over the axle
    boxes, brake cylinders, a traction motor between the wheels."""
    out = []
    for k, az in enumerate((zc - wb / 2, zc + wb / 2)):
        out += wheelset(f"{tag}_axle{k}", az, r)
        out += pair(Box(f"{tag}_axlebox{k}_right", (12.6, r - 4, az - 4.5), (18.0, r + 3, az + 4.5), "axlebox", centre=(az, r)))
        out += pair(Box(f"{tag}_spring{k}_right", (13.5, r + 3, az - 3), (17.5, r + 8, az + 3), "spring"))
    out += pair(Box(f"{tag}_frame_right", (13.0, r + 2, zc - wb / 2 - 13), (16.5, r + 8.5, zc + wb / 2 + 13), "bframe"))
    out += pair(Box(f"{tag}_frame_dip_right", (13.25, r - 1.5, zc - wb / 2 + 5), (16.25, r + 2, zc + wb / 2 - 5), "bframe"))
    out += pair(Box(f"{tag}_secondary_right", (13.5, r + 8.5, zc - 5), (16.0, top, zc + 5), "spring"))
    out.append(Box(f"{tag}_motor", (-11.0, r - 4, zc - 9), (11.0, r + 6, zc + 9), "bframe"))
    return out


def paint_transformer(c):
    if c.side:
        s = flo(c.loc[2] - c.box.frm[2])
        r = flo(c.loc[1] - c.box.frm[1])
        if s % 4 == 0:
            return FRAME_HI
        return FRAME_LO if r == 0 else FRAME
    return FRAME_LO if c.bottom else FRAME


def paint_tank(c):
    if c.side:
        r = flo(c.loc[1] - c.box.frm[1])
        n = round(c.box.to[1] - c.box.frm[1])
        return {0: FRAME_LO, n - 1: STEEL_LIGHT}.get(r, STEEL_DARK)
    return STEEL_LIGHT if c.top else STEEL_DARK


def paint_roof_kit(c):
    if c.top:
        s = flo(c.loc[2] - c.box.frm[2])
        if c.box.extra.get("grille") and 1 <= s <= c.box.to[2] - c.box.frm[2] - 2 and abs(c.x) < c.box.to[0] - 1:
            return GRILLE_DARK if s % 2 else LOUVRE_HI
        return Paint("roof", -0.1)
    return Paint("roof", -0.22)


def build_loco():
    boxes = []
    cab = loco_cab("front")
    boxes += cab
    boxes += [zmirror(b, b.name.replace("front", "rear")) for b in cab]
    boxes += loco_segment("cab_front", -LOCO_END + 10, -LOCO_MID, "cab")
    boxes += loco_segment("cab_rear", LOCO_MID, LOCO_END - 10, "cab")
    for k in range(8):
        z0 = -LOCO_MID + LOCO_BAY * k
        boxes += loco_segment(f"bay{k}", z0, z0 + LOCO_BAY, "plain" if k in (3, 4) else "bay")
    bz = LOCO_HALF - 3.6 * M  # bogies at +-98.4, as train_bodies.rs
    boxes += loco_bogie("bogie_front", -bz)
    boxes += loco_bogie("bogie_rear", bz)
    # Underframe: the transformer between the bogies, air reservoirs, skirts at the cabs.
    boxes.append(Box("transformer", (-17, 5, -44), (17, LOCO_F, 44), "transformer"))
    boxes.append(Box("reservoir_right", (13.5, 7, 46), (19.5, 13, 66), "tank"))
    boxes.append(Box("reservoir_left", (-19.5, 7, -66), (-13.5, 13, -46), "tank"))
    boxes.append(Box("skirt_front", (-LOCO_HW + 1, 9, -LOCO_END + 10), (LOCO_HW - 1, LOCO_F, -bz - 33), "loco_skirt"))
    boxes.append(Box("skirt_rear", (-LOCO_HW + 1, 9, bz + 33), (LOCO_HW - 1, LOCO_F, LOCO_END - 10), "loco_skirt"))
    # Roof: two pantographs (train_bodies.rs: 4.6 m from the ends), knees outwards; the
    # main breaker and the brake resistor housing between them.
    pz = LOCO_HALF - 4.6 * M
    boxes += pantograph("panto_front", LOCO_TOP, -pz, knee=-1)
    boxes += pantograph("panto_rear", LOCO_TOP, pz, knee=1)
    boxes.append(Box("resistor", (-12, LOCO_TOP, -40), (12, LOCO_TOP + 4, 0), "roof_kit", grille=True))
    boxes.append(Box("breaker", (-6, LOCO_TOP, 14), (6, LOCO_TOP + 3, 30), "roof_kit"))
    boxes += pair(Box("roof_line_right", (2.0, LOCO_TOP + 6.0, -pz + 10), (3.0, LOCO_TOP + 7.0, pz - 10), "panto"))
    boxes += pair(Box("line_ins_right", (1.75, LOCO_TOP, 2), (3.25, LOCO_TOP + 6.0, 4), "insulator"))
    painters = {
        "loco_flank_bay": loco_segment_painter("bay"),
        "loco_flank_plain": loco_segment_painter("plain"),
        "loco_flank_cab": loco_segment_painter("cab"),
        "loco_eave": paint_loco_eave,
        "loco_roof": paint_loco_roof,
        "loco_nose": paint_loco_nose,
        "loco_mask": paint_loco_mask,
        "loco_cabroof": paint_loco_cabroof,
        "loco_lamp": paint_loco_lamp,
        "loco_tail": paint_loco_tail,
        "beam": paint_beam,
        "buffer": paint_buffer,
        "plough": paint_plough,
        "footstep": paint_footstep,
        "transformer": paint_transformer,
        "tank": paint_tank,
        "loco_skirt": lambda c: Paint("lower", -0.3) if c.side and c.loc[1] < c.box.frm[1] + 1 else (UNDER if c.bottom else Paint("lower", -0.12)),
        "roof_kit": paint_roof_kit,
    }
    painters.update(BOGIE_PAINTERS)
    painters.update(PANTO_PAINTERS)
    return Model(
        "train_loco", boxes, painters,
        {"body": "minecraft:red_concrete", "lower": "minecraft:gray_concrete", "stripe": "minecraft:white_concrete",
         "door": "minecraft:red_concrete", "roof": "minecraft:light_gray_concrete"},
        notes="electric locomotive, 19.5 m, a cab at either end",
    )


# ---------------------------------------------------------------- freight wagons

WAGON = hexc("#3e3532")
WAGON_HI = hexc("#5b4f49")
WAGON_LO = hexc("#251f1c")
WAGON_GAP = hexc("#17130f")
BOGIE_BLACK = hexc("#2a2a2d")
BOGIE_HI = hexc("#46464c")
CASTING = hexc("#26272b")
HAZARD = hexc("#e2731e")
GRATING = hexc("#55575c")
WAGON_W = 1.4 * M            # the frame's half width, 22.4
WAGON_DECK = 1.25 * M        # 20, where loads stand


def paint_wagon_frame(c):
    """Side sills, headstocks and the open deck of a wagon."""
    if c.bottom:
        return WAGON_LO
    if c.top:
        deck = c.box.extra.get("deck")
        if deck == "walkway":
            ax = abs(c.x)
            if ax > WAGON_W - 0.6:
                return WAGON_HI
            if ax > WAGON_W - 3:
                return GRATING if (flo(c.loc[2] - c.box.frm[2]) + flo(ax)) % 2 else WAGON_LO
            return WAGON
        if not deck:
            return WAGON_HI
        ax = abs(c.x)
        if ax > WAGON_W - 2:
            return WAGON_HI
        if ax > WAGON_W - 3:
            return WAGON
        k = flo(c.loc[2] - c.box.frm[2])
        spigot = c.box.extra.get("spigots", ())
        if flo(ax) in (17, 18) and any(abs(c.z - sz) < 1.6 for sz in spigot):
            return YELLOW
        if ax < 3:
            return WAGON  # centre sill
        return WAGON if k % 17 in (0, 1) else WAGON_GAP
    r = flo(c.loc[1] - c.box.frm[1])
    n = round(c.box.to[1] - c.box.frm[1])
    if r == n - 1:
        return WAGON_HI
    if r == 0:
        return WAGON_LO
    if c.side and flo(c.loc[2] - c.box.frm[2]) % 17 == 0 and c.box.extra.get("deck") is not None:
        return WAGON_LO
    return WAGON


def paint_freight_wheel(c):
    wz, wy, r = c.box.extra["axle"]
    if c.side:
        q = math.hypot(c.z - wz, c.y - wy)
        if q < r * 0.3:
            return WHEEL_HUB
        if q < r * 0.75:
            return WHEEL_WEB
        if q < r * 0.9:
            return WHEEL_RIM
        return WHEEL_TREAD
    if c.top or c.bottom or c.front or c.back:
        return WHEEL_TREAD if abs(c.x) > 11.2 else STEEL_DARK
    return WHEEL_WEB


def paint_y25_frame(c):
    if c.top:
        return BOGIE_HI
    if c.bottom:
        return WAGON_LO
    r = flo(c.loc[1] - c.box.frm[1])
    n = round(c.box.to[1] - c.box.frm[1])
    if r == n - 1:
        return BOGIE_HI
    s = flo(c.loc[2] - c.box.frm[2])
    if c.side and r in (1, 2) and s % 6 == 3:
        return BOGIE_HI  # lightening holes' rims
    return BOGIE_BLACK


def paint_y25_axle(c):
    """An axle box with its pair of coil springs above it."""
    r = flo(c.loc[1] - c.box.frm[1])
    if c.top:
        return BOGIE_HI
    if r >= 6:
        if c.side or c.front or c.back:
            return SPRING_HI if r % 2 == 0 else SPRING_LO
    if c.side:
        dz = c.z - c.box.extra["centre"]
        if abs(dz) < 1.2 and r in (2, 3):
            return AXLEBOX_HI
        return AXLEBOX if r < 5 else BOGIE_BLACK
    return AXLEBOX


def freight_bogie(tag, zc, wb=29.0, r=7.0):
    """A Y25 freight bogie: two wheelsets, a one-piece side frame, an axle box on coil
    springs at each wheel."""
    out = []
    for k, az in enumerate((zc - wb / 2, zc + wb / 2)):
        axle = (az, r, r)
        out.append(Box(f"{tag}_axle{k}_a", (-12.6, 0, az - r * 0.55), (12.6, 2 * r, az + r * 0.55), "fwheel", axle=axle))
        out.append(Box(f"{tag}_axle{k}_b", (-12.35, r * 0.4, az - r), (12.35, 2 * r - r * 0.4, az + r), "fwheel", axle=axle))
        out += pair(Box(f"{tag}_box{k}_right", (13.25, r - 3.5, az - 3), (17.0, r + 6.5, az + 3), "y25axle", centre=az))
    out += pair(Box(f"{tag}_frame_right", (12.85, r - 1, zc - wb / 2 - 8), (15.85, r + 5, zc + wb / 2 + 8), "y25frame"))
    out.append(Box(f"{tag}_bolster", (-16.5, r + 2, zc - 4), (16.5, WAGON_DECK - 6.4, zc + 4), "y25frame"))
    return out


def paint_buffer_beam(c):
    if c.bottom:
        return WAGON_LO
    if outward(c):
        r = flo(c.loc[1] - c.box.frm[1])
        ax = abs(c.x)
        if r == round(c.box.to[1] - c.box.frm[1]) - 1:
            return WAGON_HI
        if ax > 19.5:
            return YELLOW if flo(ax + c.y) % 4 < 2 else GLOSS
        return WAGON
    return WAGON_HI if c.top else WAGON


def wagon_underframe(half, bogie_z, deck=False, spigots=()):
    """train_bodies.rs' wagon_frame: the frame from 0.3 m inside the ends, bogies, and
    buffers reaching 0.2 m past the length, so neighbours' buffers meet."""
    end = half - 0.3 * M
    # the frame in four like lengths, so their faces share the texture
    n = 4
    step = 2 * (end - 1.5) / n
    out = [Box(f"frame{k}", (-WAGON_W, 13.6, -end + 1.5 + step * k), (WAGON_W, WAGON_DECK, -end + 1.5 + step * (k + 1)),
               "wframe", deck=deck, spigots=spigots) for k in range(n)]
    for sign, tag in ((-1, "front"), (1, "rear")):
        z0, z1 = sorted((sign * (end - 1.5), sign * end))
        out.append(Box(f"{tag}_headstock", (-WAGON_W + 0.5, 12.0, z0), (WAGON_W - 0.5, WAGON_DECK - 0.5, z1), "headstock"))
        b0, b1 = sorted((sign * end, sign * (half + 2.2)))
        h0, h1 = sorted((sign * (half + 2.2), sign * (half + 3.2)))
        for x0, x1, hx0, hx1, side in ((12.0, 16.0, 10.5, 17.5, "right"), (-16.0, -12.0, -17.5, -10.5, "left")):
            out.append(Box(f"{tag}_buffer_{side}", (x0, 15.0, b0), (x1, 19.0, b1), "buffer"))
            out.append(Box(f"{tag}_buffer_head_{side}", (hx0, 13.5, h0), (hx1, 20.5, h1), "buffer", head=True))
        c0, c1 = sorted((sign * end, sign * (half + 1.6)))
        out.append(Box(f"{tag}_coupler", (-1.5, 14.0, c0), (1.5, 17.0, c1), "buffer"))
    sill = 2 * (bogie_z - 22) / 4
    out += [Box(f"centre_sill{k}", (-6.0, 8.0, -bogie_z + 22 + sill * k), (6.0, 13.6, -bogie_z + 22 + sill * (k + 1)), "wframe")
            for k in range(4)]
    out += freight_bogie("bogie_front", -bogie_z)
    out += freight_bogie("bogie_rear", bogie_z)
    return out


FREIGHT_PAINTERS = {
    "wframe": paint_wagon_frame,
    "headstock": paint_buffer_beam,
    "buffer": paint_buffer,
    "fwheel": paint_freight_wheel,
    "y25frame": paint_y25_frame,
    "y25axle": paint_y25_axle,
}


# Containers: 8 ft wide, 8.5 ft high, standing on the deck.

CT_HW = 1.22 * M        # 19.52
CT_TOP = 3.85 * M       # 61.6


def paint_container(c):
    """A corrugated steel box: top and bottom rails, corner posts with castings, doors with
    locking bars at the end `doors` names."""
    slot = c.box.extra["slot"]
    cz0, cz1 = c.box.extra["span"]
    x0, x1 = c.box.frm[0], c.box.to[0]
    y0, y1 = c.box.frm[1], c.box.to[1]
    z0 = c.box.frm[2]
    r = flo(c.loc[1] - y0)
    nr = round(y1 - y0)
    s = flo(c.loc[2] - z0)                       # along this length of the box
    from_end = min(c.loc[2] - cz0, cz1 - c.loc[2])  # from the container's nearer end
    if c.top:
        ax = abs(c.x)
        if ax > CT_HW - 1 or from_end < 1:
            return Paint(slot, 0.12)
        return Paint(slot, 0.08 if s % 6 in (0, 1) else 0.02)
    if c.bottom:
        return Paint(slot, -0.5)
    if c.side:
        corner = from_end < 2
        if corner and (r in (0, 1, nr - 2, nr - 1)):
            return CASTING
        if corner:
            return Paint(slot, -0.12)
        if r == nr - 1:
            return Paint(slot, 0.1)
        if r == nr - 2:
            return Paint(slot, -0.06)
        if r <= 1:
            return Paint(slot, -0.22)
        return Paint(slot, {0: 0.06, 1: 0.0, 2: -0.1, 3: 0.0}[(s - 2) % 4])
    # an end: doors or the corrugated end wall
    i = flo(c.loc[0] - x0)
    ni = round(x1 - x0)
    corner = i in (0, 1, ni - 2, ni - 1)
    if corner and r in (0, 1, nr - 2, nr - 1):
        return CASTING
    if corner:
        return Paint(slot, -0.12)
    if r in (0, 1, nr - 2, nr - 1):
        return Paint(slot, -0.06 if r in (nr - 2, 1) else 0.04)
    doors = c.box.extra["doors"]
    if (c.front and doors < 0) or (c.back and doors > 0):
        mid = ni // 2
        if i in (mid - 1, mid):
            return Paint(slot, -0.35) if i == mid - 1 else Paint(slot, 0.05)
        # two locking bars per leaf, with their handles
        k = (i - 2) % ((mid - 2) // 2 if mid > 6 else 4)
        if i in (5, 6, mid - 5, mid - 4, mid + 3, mid + 4, ni - 7, ni - 6):
            if r in (12, 13) and i in (6, mid - 4, mid + 4, ni - 6):
                return CHROME_DARK
            return Paint(slot, -0.28) if i in (5, mid - 5, mid + 3, ni - 7) else STEEL_LIGHT
        if r in (7, nr - 8):
            return Paint(slot, -0.2)  # hinges
        return Paint(slot, 0.02)
    return Paint(slot, {0: 0.06, 1: 0.0, 2: -0.1, 3: 0.0}[i % 4])


def container(tag, z0, z1, slot, doors, n):
    """A container in n like lengths, so the middle ones share their texture."""
    step = (z1 - z0) / n
    return [Box(f"{tag}{k}", (-CT_HW, WAGON_DECK, z0 + step * k), (CT_HW, CT_TOP, z0 + step * (k + 1)), "container",
                slot=slot, doors=doors, span=(z0, z1)) for k in range(n)]


CONTAINER_HALF = 18.4 / 2 * M   # 147.2
CONTAINER_BOGIE = CONTAINER_HALF - 2.9 * M


def freight_model(id, boxes, painters, livery, notes):
    p = dict(FREIGHT_PAINTERS)
    p.update(painters)
    return Model(id, boxes, p, livery, notes=notes)


def build_container_40():
    boxes = wagon_underframe(CONTAINER_HALF, CONTAINER_BOGIE, deck=True, spigots=(-97.6, 97.6))
    boxes += container("container", -6.1 * M, 6.1 * M, "body", 1, 5)
    return freight_model("freight_container_40", boxes, {"container": paint_container},
                         {"body": "minecraft:blue_concrete"},
                         "container wagon, 18.4 m, one 40 ft container (variants 0-3)")


def build_container_2x20():
    boxes = wagon_underframe(CONTAINER_HALF, CONTAINER_BOGIE, deck=True, spigots=(-101.6, -4, 4, 101.6))
    # train_bodies.rs' first container (-6.35..-0.25 m there) is our rear one
    boxes += container("container_rear", 0.25 * M, 6.35 * M, "body", 1, 3)
    boxes += container("container_front", -6.35 * M, -0.25 * M, "lower", -1, 3)
    return freight_model("freight_container_2x20", boxes, {"container": paint_container},
                         {"body": "minecraft:orange_concrete", "lower": "minecraft:cyan_terracotta"},
                         "container wagon, 18.4 m, two 20 ft containers (variants 5-9)")


def build_flat():
    boxes = wagon_underframe(CONTAINER_HALF, CONTAINER_BOGIE, deck=True, spigots=(-101.6, -97.6, -4, 4, 97.6, 101.6))
    return freight_model("freight_flat", boxes, {}, {"body": "minecraft:gray_concrete"},
                         "container wagon, 18.4 m, empty (variant 4)")


# Tank wagon: an octagonal barrel of four boxes turned about its axis.

TANK_HALF = 15.0 / 2 * M     # 120
TANK_BOGIE = TANK_HALF - 2.9 * M
TANK_Y = 40.0                # the barrel's axis, 2.5 m up
TANK_R = 20.0                # to its flats
TANK_FLAT = TANK_R * math.tan(math.radians(22.5))
TANK_LEN = TANK_HALF - 0.7 * M   # its ends, as train_bodies.rs


def octagon_r(x, y):
    """Distance from the barrel's axis, measured square to the nearest flat."""
    dy = y - TANK_Y
    return max(abs(x), abs(dy), (abs(x) + abs(dy)) / math.sqrt(2))


def paint_tank_barrel(c):
    nx, ny = c.n[0], c.n[1]
    if abs(c.n[2]) > 0.5:
        # the dished end: rings round the middle, a weld near the rim
        q = octagon_r(c.x, c.y) / TANK_R
        if q > 0.93:
            return Paint("body", -0.16)
        if 0.78 < q <= 0.84:
            return Paint("body", -0.08)
        return Paint("body", 0.06 if q < 0.45 else 0.0)
    k = 0.16 if ny > 0.9 else 0.07 if ny > 0.5 else -0.15 if ny < -0.5 else 0.0
    if ny < -0.9:
        k = -0.3
    z = c.loc[2]
    if flo(abs(z)) in (36, 72) :
        return Paint("body", k - 0.12)  # weld seams between the barrel's rings
    if abs(nx) > 0.9 and ny == 0:
        # the danger placard by one end, orange with a black bar
        ry = flo(c.y - (TANK_Y - TANK_FLAT))
        if 84 <= z < 98 and 3 <= ry <= 12:
            if ry in (3, 12) or flo(z) in (84, 97):
                return GLOSS
            return GLOSS if ry == 7 else HAZARD
    return Paint("body", k)


def paint_ladder(c):
    if outward(c):
        r = flo(c.loc[1] - c.box.frm[1])
        i = flo(c.loc[0] - c.box.frm[0])
        w = round(c.box.to[0] - c.box.frm[0])
        if i in (0, w - 1):
            return STEEL_LIGHT
        return STEEL_LIGHT if r % 3 == 0 else WAGON_GAP
    return STEEL_DARK


def paint_dome(c):
    if c.top:
        return Paint("body", 0.2) if c.box.extra.get("lid") else Paint("body", 0.12)
    r = flo(c.loc[1] - c.box.frm[1])
    return Paint("body", -0.18 if r == 0 else -0.02)


def paint_platform(c):
    if c.top:
        i = flo(c.loc[0] - c.box.frm[0]) + flo(c.loc[2] - c.box.frm[2])
        return GRATING if i % 2 else WAGON_LO
    return STEEL_DARK


def build_tank():
    boxes = wagon_underframe(TANK_HALF, TANK_BOGIE, deck="walkway")
    L = TANK_LEN
    o = (0, TANK_Y, 0)
    # The flats' ends stand 0.25 apart, so they never share a plane: a shallow dish. Each
    # flat in two halves, so no face is too long to pack well.
    for half, sign in (("front", -1), ("rear", 1)):
        def span(inset):
            return sorted((0.0, sign * (L - inset)))
        z0, z1 = span(0)
        boxes.append(Box(f"barrel_tall_{half}", (-TANK_FLAT, TANK_Y - TANK_R, z0), (TANK_FLAT, TANK_Y + TANK_R, z1), "barrel"))
        z0, z1 = span(0.25)
        boxes.append(Box(f"barrel_wide_{half}", (-TANK_R, TANK_Y - TANK_FLAT, z0), (TANK_R, TANK_Y + TANK_FLAT, z1), "barrel"))
        z0, z1 = span(0.5)
        boxes.append(Box(f"barrel_diag_a_{half}", (-TANK_FLAT, TANK_Y - TANK_R, z0), (TANK_FLAT, TANK_Y + TANK_R, z1), "barrel",
                         origin=o, rot=(0, 0, 45)))
        z0, z1 = span(0.75)
        boxes.append(Box(f"barrel_diag_b_{half}", (-TANK_FLAT, TANK_Y - TANK_R, z0), (TANK_FLAT, TANK_Y + TANK_R, z1), "barrel",
                         origin=o, rot=(0, 0, -45)))
    top = TANK_Y + TANK_R
    boxes.append(Box("dome", (-6, top - 0.5, -7), (6, top + 3.5, 7), "dome"))
    boxes.append(Box("dome_lid", (-4.5, top + 3.5, -5), (4.5, top + 4.5, 5), "dome", lid=True))
    boxes.append(Box("top_walk", (-4.0, top, -26), (4.0, top + 0.5, 26), "platform"))
    for sign, tag in ((-1, "front"), (1, "rear")):
        z0, z1 = sorted((sign * L, sign * (L + 1.0)))
        boxes.append(Box(f"{tag}_ladder", (-3.0, WAGON_DECK, z0), (3.0, top - 2, z1), "ladder"))
    return freight_model("freight_tank", boxes,
                         {"barrel": paint_tank_barrel, "dome": paint_dome, "ladder": paint_ladder, "platform": paint_platform},
                         {"body": "minecraft:light_gray_concrete"}, "tank wagon, 15 m")


# Boxcar: ribbed sides, a rounded roof, a sliding door each side.

BOX_HALF = 16.5 / 2 * M      # 132
BOX_BOGIE = BOX_HALF - 2.9 * M
BOX_END = BOX_HALF - 0.35 * M    # 126.4
BOX_WALL = 60.0
BOX_EAVE = 62.4
BOX_TOP = 65.6
BOX_DOOR = 1.6 * M               # half the door's width
DOOR_COLOUR = (82, 66, 60)       # gray terracotta, as train_bodies.rs


def paint_boxcar_side(c):
    if c.bottom:
        return WAGON_LO
    if c.top:
        return Paint("body", 0.14)
    r = flo(c.loc[1] - c.box.frm[1])
    nr = round(c.box.to[1] - c.box.frm[1])
    if c.side:
        s = flo(c.loc[2] - c.box.frm[2])
        if r <= 1:
            return Paint("body", -0.32 if r == 0 else -0.2)
        if r >= nr - 2:
            return Paint("body", 0.12 if r == nr - 1 else -0.06)
        # an outside stanchion every 16 units, lit on one edge
        if s % 16 == 0:
            return Paint("body", 0.1)
        if s % 16 == 1:
            return Paint("body", -0.18)
        return Paint("body", 0.0)
    # the ends: ribs across
    if r <= 1:
        return Paint("body", -0.3)
    return Paint("body", -0.14 if r % 10 == 0 else -0.04)


def paint_boxcar_eave(c):
    if c.side:
        return Paint("body", 0.1)
    if c.top:
        return Paint("body", 0.18)
    return Paint("body", 0.0)


def paint_boxcar_roof(c):
    if c.top:
        s = flo(c.loc[2] - c.box.frm[2])
        return Paint("body", 0.06 if s % 16 == 0 else 0.14)
    return Paint("body", 0.1)


def paint_slide_door(c):
    door = DOOR_COLOUR
    if c.side:
        s = flo(c.loc[2] - c.box.frm[2])
        n = round(c.box.to[2] - c.box.frm[2])
        r = flo(c.loc[1] - c.box.frm[1])
        nr = round(c.box.to[1] - c.box.frm[1])
        mid = n // 2
        if s in (0, n - 1, mid - 1) or r in (0, nr - 1):
            return mix(door, (0, 0, 0), 0.4)
        if s == mid:
            return mix(door, (255, 255, 255), 0.1)
        # locking bars and a handle on each leaf
        if s in (4, n - 5):
            return STEEL_LIGHT if r % 12 else STEEL_DARK
        if s in (mid - 3, mid + 2) and 15 <= r <= 18:
            return STEEL_DARK
        if r % 9 == 0:
            return mix(door, (0, 0, 0), 0.18)
        return door
    if c.top:
        return mix(door, (255, 255, 255), 0.12)
    return mix(door, (0, 0, 0), 0.2)


def paint_rail(c):
    if c.top:
        return STEEL_LIGHT
    return STEEL_DARK if c.side else WAGON_LO


def build_boxcar():
    boxes = wagon_underframe(BOX_HALF, BOX_BOGIE)
    n = 4
    step = 2 * BOX_END / n
    for k in range(n):
        z0, z1 = -BOX_END + step * k, -BOX_END + step * (k + 1)
        boxes.append(Box(f"body{k}", (-WAGON_W, WAGON_DECK, z0), (WAGON_W, BOX_WALL, z1), "boxcar_side"))
        boxes.append(Box(f"body{k}_eave", (-WAGON_W + 1, BOX_WALL, z0), (WAGON_W - 1, BOX_EAVE, z1), "boxcar_eave"))
        boxes.append(Box(f"body{k}_roof", (-WAGON_W + 3, BOX_EAVE, z0), (WAGON_W - 3, BOX_TOP, z1), "boxcar_roof"))
    boxes += pair(Box("door_right", (WAGON_W, WAGON_DECK + 1.5, -BOX_DOOR), (WAGON_W + 0.75, BOX_WALL - 1.5, BOX_DOOR), "slide_door"))
    boxes += pair(Box("door_rail_top_right", (WAGON_W, BOX_WALL - 1.5, -2 * BOX_DOOR - 4), (WAGON_W + 1.25, BOX_WALL, BOX_DOOR + 2), "rail"))
    boxes += pair(Box("door_rail_low_right", (WAGON_W, WAGON_DECK, -2 * BOX_DOOR - 4), (WAGON_W + 1.25, WAGON_DECK + 1.5, BOX_DOOR + 2), "rail"))
    return freight_model("freight_boxcar", boxes,
                         {"boxcar_side": paint_boxcar_side, "boxcar_eave": paint_boxcar_eave,
                          "boxcar_roof": paint_boxcar_roof, "slide_door": paint_slide_door, "rail": paint_rail},
                         {"body": "minecraft:brown_terracotta"}, "boxcar, 16.5 m, sliding doors")


MODELS = [
    build_ice_cab, build_ice_mid, build_loco,
    build_container_40, build_container_2x20, build_flat, build_tank, build_boxcar,
]
