"""Airliners: a single-aisle twinjet of the A320/737 class (`plane_narrow`, 37.6 m long,
35.8 m span) and a twin-aisle twinjet of the A350/787 class (`plane_wide`, 65 m, 63 m).

The aircraft frame: nose towards -z, the right wing at +x, the wheels standing on y = 0
and z = 0 at the main gear's axle line, which the display pitches about for rotation and
the flare. Every landing gear box is in a `gear*` bone, so the kit writes `<id>_up` too.

Livery: `body` the fuselage, `lower` its belly, `stripe` the cheatline under the windows,
`door` the fin and the wingtip devices, `roof` the engine nacelles. Wings, tailplanes,
fairings and gear are fixed greys.

The fuselage is a run of sections, each a stack of nested boxes (a stepped round); only
the widest carries the window bays, repeated so their faces share one spot of the texture.
Wings, tailplanes and the fin are each two families of slabs: one turned to the leading
edge's sweep, a thinner one turned to the trailing edge's, so both edges come out straight.

Riders: the cabin is one shell from the first door to the last (`cabin` sections), with
nothing in it, so a rider anywhere along its middle at window height looks out freely.
Seats belong between the first and the last door, not behind the last one.
"""
import math
from types import SimpleNamespace

from kit import (
    GLASS_DARK, GLASS_MID, GLOSS, TIRE, TIRE_SIDE, Box, Model, Paint, glass, hexc,
)

# ---------------------------------------------------------------- colours

WING = hexc("#b3b9c0")
WING_HI = hexc("#d2d6db")
WING_EDGE = hexc("#c3c8ce")
WING_LO = hexc("#99a0a8")
WING_GAP = hexc("#737a83")
WING_PANEL = hexc("#a9b0b8")
FLAP = hexc("#a7aeb6")
FLAP_HI = hexc("#bcc2c9")
FAIRING = hexc("#aab0b8")
FAIRING_HI = hexc("#bfc5cb")
FAIRING_LO = hexc("#8c939b")
LIP = hexc("#e3e6ea")
LIP_LO = hexc("#b9bec5")
FAN = hexc("#1e2126")
FAN_BLADE = hexc("#3b4047")
SPINNER = hexc("#c2c7cd")
DUCT = hexc("#2b2e34")
HOT = hexc("#6b6f76")
HOT_HI = hexc("#8a8f97")
HOT_LO = hexc("#4a4d53")
STRUT = hexc("#a3a9b0")
STRUT_LO = hexc("#7b8189")
OLEO = hexc("#dfe3e7")
HUB = hexc("#c9ced3")
HUB_LO = hexc("#868c94")
WELL = hexc("#3a3e45")
LAMP_RED = hexc("#ff3a34")
LAMP_GREEN = hexc("#35f06a")
LAMP_WHITE = hexc("#fffbea")
LAMP_LAND = hexc("#fff6d8")
WIN_LO = hexc("#18222e")


def flo(v):
    return math.floor(v + 1e-6)


def half(v):
    return round(v * 2) / 2


def darker(p, d):
    return Paint(p.slot, p.k - d) if isinstance(p, Paint) else tuple(round(v * (1 - d)) for v in p)


class Parts:
    """Collects boxes, naming each uniquely."""

    def __init__(self):
        self.boxes = []
        self._count = {}

    def _name(self, base):
        k = self._count.get(base, 0)
        self._count[base] = k + 1
        return f"{base}_{k}" if k else base

    def add(self, base, frm, to, mat, **kw):
        b = Box(self._name(base), frm, to, mat, **kw)
        self.boxes.append(b)
        return b

    def pair(self, base, frm, to, mat, **kw):
        """A box on the right (+x) and its mirror image on the left."""
        b = self.add(base + "_r", frm, to, mat, **kw)
        self.boxes.append(b.mirrored(self._name(base + "_l")))
        return b


# ---------------------------------------------------------------- fuselage

# A section's nested boxes, widest first: (share of the half width, share of the height
# above and below the waist).
FRACS = ((1.0, 0.45), (0.85, 0.72), (0.6, 0.91), (0.31, 1.0))
FRACS_MID = ((1.0, 0.5), (0.76, 0.82), (0.42, 1.0))
FRACS_SMALL = ((1.0, 0.6), (0.6, 1.0))


def clamp_layer(layer, prev):
    """The layer cut down to lie inside one of the neighbouring section's layers, so the
    face towards the cabin is wholly inside that section and left out; and which one."""
    hw, lo, hi = layer
    best = None
    for j, (pw, plo, phi) in enumerate(prev):
        c = (min(hw, pw), max(lo, plo), min(hi, phi))
        area = c[0] * (c[2] - c[1])
        if best is None or area > best[0]:
            best = (area, c, j)
    return best[1], best[2]


def section_layers(w, b, t, yc, prev=None):
    """A section's layers (half width, y0, y1), widest first, and for each the layer of
    `prev` it lies inside."""
    out, into = [], []
    for fw, fh in (FRACS if w >= 24 else FRACS_MID if w >= 12 else FRACS_SMALL):
        layer, j = (half(w * fw), half(yc - fh * (yc - b)), half(yc + fh * (t - yc))), None
        if prev:
            layer, j = clamp_layer(layer, prev)
        if layer[2] - layer[1] <= 0:
            continue
        if any(o[0] >= layer[0] and o[1] <= layer[1] and o[2] >= layer[2] for o in out):
            continue
        out.append(layer)
        into.append(j)
    return out, into


STAGGER = 0.25


def build_fuselage(P, S):
    """The sections' boxes; returns each section's layers. A section's layers end a
    quarter unit apart at its free end, so their end faces never share a plane, and each
    begins where the layer of the neighbour it lies inside ends.

    The cabin is the main section and the `cabin` sections behind it. They share one
    shell: a rider anywhere in it is inside every box of it, so sees none. Only the keel
    under it steps up from one to the next, as low slabs whose end faces lie far below
    the windows."""
    secs = S.sections
    main = next(i for i, s in enumerate(secs) if s.get("main"))
    n = len(secs)
    yc = secs[main]["yc"]
    cab = [i for i in range(n) if i == main or secs[i].get("cabin")]
    c0, c1 = min(cab), max(cab)
    assert c0 == main and cab == list(range(c0, c1 + 1))
    lay, into = [None] * n, [None] * n
    front, rear = [None] * n, [None] * n
    for i in [main] + list(range(main - 1, -1, -1)) + list(range(main + 1, n)):
        s = secs[i]
        near = None if i == main else (i + 1 if i < main else i - 1)
        if i != main and i in cab:
            m = lay[main]
            assert len(m) == 4
            l2 = max(m[2][1], half(yc - FRACS[2][1] * (yc - s["b"])))
            lay[i] = [m[0], m[1], (m[2][0], l2, m[2][2]), (m[3][0], max(m[3][1], s["b"]), m[3][2])]
            into[i] = [0, 1, 2, 3]
        else:
            lay[i], into[i] = section_layers(s["w"], s["b"], s["t"], s["yc"], lay[near] if near is not None else None)
        ks = range(len(lay[i]))
        front[i] = [s["z0"] + STAGGER * k if i <= main else rear[near][into[i][k]] for k in ks]
        rear[i] = [s["z1"] - STAGGER * k if i >= main else front[near][into[i][k]] for k in ks]
    mats = {"plain": "skin", "bay": "bay", "door": "door", "exit": "exit"}
    for i, s in enumerate(secs):
        segs = s.get("segs") or [("plain", s["z1"] - s["z0"])]
        assert abs(sum(m for _, m in segs) - (s["z1"] - s["z0"])) < 1e-6, (i, segs)
        for k, (hw, lo, hi) in enumerate(lay[i]):
            if i in cab and k >= 2:
                if i == c0:
                    # From just inside the second layer's underside up, one box runs the
                    # cabin's length. Under it the keel: a slab for each level the belly
                    # steps up through, reaching back over the sections that deep, its
                    # top a quarter unit inside what lies over it; every other slab is a
                    # quarter unit narrower, so their sides do not share a plane.
                    floor = lay[c0][1][1]
                    P.add(f"fus_crown_{k}", (-hw, floor + 0.25, front[c0][k]), (hw, hi, rear[c1][k]), "skin",
                          z0=s["z0"], z1=secs[c1]["z1"])
                    levels = sorted({min(lay[j][k][1], floor) for j in cab} | {floor}, reverse=True)
                    for q, (top, bottom) in enumerate(zip(levels, levels[1:])):
                        last = max(j for j in cab if lay[j][k][1] <= bottom + 1e-6)
                        kw = hw - 0.25 * (q % 2)
                        P.add(f"fus_keel_{k}", (-kw, bottom, front[c0][k]), (kw, top + (0.5 if q == 0 else 0.25), rear[last][k]),
                              "skin", z0=s["z0"], z1=secs[last]["z1"])
                continue
            runs = []
            z = s["z0"]
            for kind, m in segs:
                if k >= 2 or (k == 1 and kind != "door"):
                    kind = "plain"
                if runs and kind == "plain" and runs[-1][0] == "plain":
                    runs[-1][2] = z + m
                else:
                    runs.append([kind, z, z + m])
                z += m
            for r, (kind, za, zb) in enumerate(runs):
                fa = front[i][k] if r == 0 else za
                fb = rear[i][k] if r == len(runs) - 1 else zb
                P.add(f"fus{i}_{k}_{kind}", (-hw, lo, fa), (hw, hi, fb), mats[kind], z0=za, z1=zb)
    return lay


def fuselage_painters(S, lay_main):
    """Painters of the fuselage skin, the window bays and the doors."""
    y_up2, y_up1 = lay_main[1][2], lay_main[0][2]
    y_dn1, y_dn2 = lay_main[1][1], lay_main[2][1]
    s0, s1 = S.stripe
    w0, w1 = S.win_rows

    tail_z = S.sections[-1]["z1"]

    def skin(c):
        y, z = c.y, c.z
        iy = flo(y)
        if c.back and z > tail_z - 0.5:
            return HOT_LO
        if c.bottom:
            p = Paint("lower", -0.2) if y < S.yc else Paint("body", -0.12)
        elif c.top:
            p = Paint("body", 0.14) if y > S.yc else Paint("lower", 0.06)
        else:
            k = 0.04 if c.front else (-0.06 if c.back else 0.0)
            if iy < S.belly:
                k += -0.16 if y < y_dn2 else (-0.1 if y < y_dn1 else -0.03)
                p = Paint("lower", k)
            elif s0 <= iy < s1 and S.stripe_z[0] <= c.box.extra.get("z0", z) < S.stripe_z[1]:
                p = Paint("stripe", k + (0.1 if iy == s1 - 1 else 0.0))
            else:
                k += 0.1 if y >= y_up2 else (0.05 if y >= y_up1 else 0.0)
                p = Paint("body", k)
        if not (c.front or c.back) and S.radome_z <= z < S.radome_z + 1:
            return darker(p, 0.22)
        return p

    def bay(c):
        if c.side:
            m = flo(c.loc[2] - c.box.extra["z0"]) % S.win_pitch
            iy = flo(c.y)
            a = (S.win_pitch - S.win_w) // 2
            if w0 <= iy < w1 and a <= m < a + S.win_w:
                if m in (a, a + S.win_w - 1) and iy in (w0, w1 - 1):
                    return Paint("body", -0.14)
                if iy == w1 - 1:
                    return GLASS_MID
                return WIN_LO if iy == w0 else GLASS_DARK
        return skin(c)

    def door(c):
        if c.side:
            n = round(c.box.extra["z1"] - c.box.extra["z0"])
            s = flo(c.loc[2] - c.box.extra["z0"])
            iy = flo(c.y)
            d0, d1 = S.door_rows
            if d0 <= iy < d1 and 1 <= s < n - 1:
                if s in (1, n - 2) or iy in (d0, d1 - 1):
                    return darker(skin(c), 0.4)
                if s in (n // 2 - 1, n // 2) and w0 + 1 <= iy < w1:
                    return GLASS_MID if iy == w1 - 1 else GLASS_DARK
                if iy == d0 + 1:
                    return darker(skin(c), 0.12)
        return skin(c)

    def exit_bay(c):
        """A window bay whose middle windows are overwing exits: an outline round each."""
        if c.side:
            n = round(c.box.extra["z1"] - c.box.extra["z0"])
            s = flo(c.loc[2] - c.box.extra["z0"])
            iy = flo(c.y)
            e0, e1 = S.exit_rows
            for a in S.exit_at:
                for lo in (a, n - a - S.win_pitch):
                    k = s - lo
                    if 0 <= k < S.win_pitch and e0 <= iy < e1 and (k in (0, S.win_pitch - 1) or iy in (e0, e1 - 1)):
                        if not (w0 <= iy < w1 and 0 < k < S.win_pitch - 1):
                            return darker(skin(c), 0.38)
        return bay(c)

    return {"skin": skin, "bay": bay, "door": door, "exit": exit_bay}


# ---------------------------------------------------------------- cockpit


def build_cockpit(P, S):
    """The raked windscreen on a stair of level steps (its underside stays inside them,
    out of a rider's view), the block behind it with the corner and side panes, a brow."""
    K = S.cockpit
    z0, y0, slope = K["z0"], K["y0"], K["slope"]
    hw, step = K["hw"], K["step"]
    zr = K["z_rear"]
    line = lambda y: z0 + (y - y0) / slope
    drop = K.get("drop", 8)
    P.add("ws_sill", (-hw, y0 - drop, z0 + 1.0), (hw, y0, K["block_z"]), "ws_stair")
    y = y0
    while y < K["y_top"] - 1e-6:
        P.add("ws_step", (-hw, y, line(y + step)), (hw, y + step, zr), "ws_stair")
        y += step
    ang = math.degrees(math.atan(slope))
    length = (K["y_top"] - 0.2 - y0) / math.sin(math.radians(ang))
    ys, zs = y0 - 0.4 * slope, z0 - 0.4
    thick = K.get("thick", 2.5)
    P.add(
        "windscreen", (-hw + 2.5, ys - thick, zs), (hw - 2.5, ys, zs + length),
        "windscreen", origin=(0, ys, zs), rot=(-ang, 0, 0),
    )
    P.add("brow", (-hw + 0.5, K["y_top"] - 1.5 * step, line(K["y_top"]) - 1), (hw - 0.5, K["brow_top"], zr), "brow")
    bw, bt = K["block_hw"], K["block_top"]
    P.add("cockpit", (-bw, y0 - drop + 2, K["block_z"]), (bw, bt, zr), "cockpit")
    P.add("cockpit_top", (-K["top_hw"], bt, line(K["y_top"]) + 0.5), (K["top_hw"], K["y_top"], zr), "brow")


def cockpit_painters(S, skin):
    K = S.cockpit
    g0, g1 = K["glass"]
    y0 = K["y0"]

    def pane(c, a, b):
        return glass(c.y, a, b, top=g1, bottom=g0)

    def stair(c):
        # The steps under the windscreen: its dark frame, and glazed cheeks.
        if c.side and c.y >= g0:
            if c.y < g0 + 1 or K["block_z"] - 1 < c.z < K["block_z"]:
                return GLOSS
            return glass(c.y, c.z, c.y, top=K["y_top"], bottom=g0)
        if c.y < y0 and (c.front or c.side):
            return skin(c)
        return GLOSS

    def windscreen(c):
        if c.face == "up":
            x, s = c.loc[0], c.loc[2] - c.box.frm[2]
            n = c.box.to[2] - c.box.frm[2]
            if abs(x) > c.box.to[0] - 1 or abs(x) < 0.6 or s < 1 or s > n - 1:
                return GLOSS
            return glass(c.y, x, s, top=K["y_top"], bottom=y0)
        return GLOSS

    def block(c):
        y, z = c.y, c.z
        if g0 <= y < g1:
            if c.front:
                ax = abs(c.x)
                if ax > c.box.to[0] - 1 or y < g0 + 1 or y >= g1 - 1:
                    return GLOSS
                return pane(c, c.x, y)
            if c.side and z < K["side_z"]:
                if y < g0 + 1 or y >= g1 - 1 or z < K["block_z"] + 1 or z >= K["side_z"] - 1:
                    return GLOSS
                if flo(z) == flo(K["post_z"]):
                    return GLOSS
                return pane(c, z, y)
        return skin(c)

    return {"ws_stair": stair, "windscreen": windscreen, "cockpit": block, "brow": skin}


# ---------------------------------------------------------------- wings, tail


def interp(pts, u):
    """z along a polyline of (u, z) points, and its slope there."""
    for (ua, za), (ub, zb) in zip(pts, pts[1:]):
        if u <= ub + 1e-6 or (ub, zb) == pts[-1]:
            k = (zb - za) / (ub - ua)
            return za + (u - ua) * k, k
    raise ValueError(u)


def flush_chord(le, te, ub):
    """For a surface's last stretch ending at ub: the trailing slab's chord that puts its
    forward outer corner on the leading edge, and how far out that corner lies."""
    zt, k = interp(te, ub)
    tl = math.tan(math.radians(le[2]))
    cb = zt - (le[1] + (ub - le[0]) * tl)
    a = math.atan(k)
    c = cb / (math.cos(a) * (math.cos(a) + math.sin(a) * tl))
    return c, ub + c * math.cos(a) * math.sin(a)


def flush_end(le, te, top):
    """Where the last stretch must end for the surface to reach `top`."""
    lo, hi = top - 60.0, top
    for _ in range(40):
        mid = (lo + hi) / 2
        if flush_chord(le, te, mid)[1] < top:
            lo = mid
        else:
            hi = mid
    return lo


def lifting(P, name, axis, le, te, segs, mat_lead, mat_trail, thin=(0.5, 0.75), lead=(0.92, 0.62),
            trim=False, flush=False, lead_from=None, **kw):
    """A tapered, swept surface as two families of slabs: one along the leading edge, a
    thinner one along the trailing edge. `axis` "x": a wing reaching out along x (built
    on the right and mirrored), slabs turned about y; "y": a fin standing up, turned
    about x. `le` is (u, z, sweep in degrees) of the leading edge, `te` the trailing
    edge's (u, z) points, `segs` the stretches (u0, u1, lo, hi) with their extent across.
    `flush` ends the surface in a clean tip: the last trailing slab's forward corner lies
    on the leading edge and the leading slab runs out to it. `trim` instead keeps that
    corner inside the span, for a tip that gets a cap. `lead_from` starts the first
    leading slab further out, so its slanted root end stays on its own side."""
    u_ref, z_ref, sweep = le
    tl = math.tan(math.radians(sweep))
    cl = math.cos(math.radians(sweep))
    for i, (ua, ub, lo, hi) in enumerate(segs):
        last = i == len(segs) - 1
        za, ka = interp(te, (ua + ub) / 2)
        zta = za - ka * (ub - ua) / 2
        ztb = za + ka * (ub - ua) / 2
        ca = zta - (z_ref + (ua - u_ref) * tl)
        cb = ztb - (z_ref + (ub - u_ref) * tl)
        c_lead = min(lead[0] * cb, lead[1] * ca)
        c_trail = ca - c_lead + 3.0
        ang_t = math.degrees(math.atan(ka))
        ct = math.cos(math.radians(ang_t))
        run = (ub - ua) / ct
        la, lb = ua, ub
        if i == 0 and lead_from is not None:
            la = lead_from
        if flush and last:
            c_trail, lb = flush_chord(le, te, ub)
            assert c_lead + c_trail >= ca + 1, (name, c_lead, c_trail, ca)
        else:
            assert c_trail <= cb + 2.5, (name, i, c_trail, cb)
        if trim and last:
            # keep the slab's forward outer corner inside the span
            run -= c_trail * ct * math.tan(math.radians(ang_t))
        sa, sb = (la - u_ref) / cl, (lb - u_ref) / cl
        if axis == "x":
            P.pair(f"{name}_lead{i}", (u_ref + sa, lo, z_ref), (u_ref + sb, hi, z_ref + c_lead * cl), mat_lead,
                   origin=(u_ref, 0, z_ref), rot=(0, -sweep, 0), **kw)
            P.pair(f"{name}_trail{i}", (ua, lo + thin[0], zta - c_trail * ct), (ua + run, hi - thin[1], zta),
                   mat_trail, origin=(ua, 0, zta), rot=(0, -ang_t, 0), **kw)
        else:
            P.add(f"{name}_lead{i}", (lo, u_ref + sa, z_ref), (hi, u_ref + sb, z_ref + c_lead * cl), mat_lead,
                  origin=(0, u_ref, z_ref), rot=(sweep, 0, 0), **kw)
            P.add(f"{name}_trail{i}", (lo + thin[0], ua, zta - c_trail * ct), (hi - thin[0], ua + run, zta),
                  mat_trail, origin=(0, ua, zta), rot=(ang_t, 0, 0), **kw)


def paint_lead(c):
    """A wing's forward slab: a lighter leading edge, a seam where the flaps begin."""
    if c.face == "up":
        q = c.loc[2] - c.box.frm[2]
        n = c.box.to[2] - c.box.frm[2]
        if q < 3:
            return WING_HI
        if q < 4 or q > n - 1:
            return WING_GAP
        if n > 24 and q > n - 1 - n * 0.22:
            return WING_PANEL
        return WING
    if c.face == "down":
        return WING_LO
    if c.face == "north":
        return WING_HI
    return WING_EDGE


def paint_trail(c):
    if c.face == "up":
        return FLAP_HI if c.loc[2] > c.box.to[2] - 1 else FLAP
    if c.face == "down":
        return WING_LO
    return WING_EDGE


def paint_fairing(c):
    if c.top:
        return FAIRING_HI
    if c.bottom:
        return FAIRING_LO
    y = c.y
    if y > c.box.to[1] - 1:
        return FAIRING_HI
    if y < c.box.frm[1] + 1:
        return FAIRING_LO
    return FAIRING


def build_wing(P, S):
    W = S.wing
    lifting(P, "wing", "x", W["le"], W["te"], W["segs"], "wing_lead", "wing_trail", trim=True)
    tl = math.tan(math.radians(W["le"][2]))
    S.wing_le = lambda x: W["le"][1] + (x - W["le"][0]) * tl
    S.wing_te = lambda x: interp(W["te"], x)[0]
    for x in W["tracks"]:
        lo = next(s[2] for s in W["segs"] if s[0] <= x <= s[1])
        zt = S.wing_te(x)
        w, h, n = W["track_size"]
        P.pair("flap_track", (x - w, lo - h, zt - n), (x + w, lo + 0.5, zt + n * 0.3), "fairing")
    for frm, to in W["fairing"]:
        P.add("wing_fairing", frm, to, "fairing")


# ---------------------------------------------------------------- engines


def build_engines(P, S):
    E = S.engine
    ex, ey, R = E["x"], E["y"], E["r"]
    z0, z1, z2, z3, z4 = E["z"]          # lip, cowl end, core end, nozzle end, plug end
    lr, lt, ld = E["lip_r"], E["lip_t"], E["lip_d"]
    ri = lr - lt
    # the intake: a ring of bars and corner blocks round the fan, which sits back in it
    c, k = E["lip_c"], E["lip_k"]
    ext = dict(engine=(ex, ey))
    P.pair("eng_lip_top", (ex - c, ey + ri, z0), (ex + c, ey + lr, z0 + ld), "eng_lip", **ext)
    P.pair("eng_lip_bottom", (ex - c, ey - lr, z0), (ex + c, ey - ri, z0 + ld), "eng_lip", **ext)
    P.pair("eng_lip_out", (ex + ri, ey - c, z0), (ex + lr, ey + c, z0 + ld), "eng_lip", **ext)
    P.pair("eng_lip_in", (ex - lr, ey - c, z0), (ex - ri, ey + c, z0 + ld), "eng_lip", **ext)
    for sx in (-1, 1):
        for sy in (-1, 1):
            x0, x1 = sorted((ex + sx * c, ex + sx * k))
            y0, y1 = sorted((ey + sy * c, ey + sy * k))
            P.pair("eng_lip_corner", (x0, y0, z0 + 0.25), (x1, y1, z0 + ld), "eng_lip", **ext)
    P.pair("eng_fan", (ex - ri, ey - ri, z0 + ld * 0.6), (ex + ri, ey + ri, z0 + ld), "eng_fan", **ext)
    a, b = E["oct"]
    zc = z0 + ld
    P.pair("eng_cowl_wide", (ex - R, ey - a, zc + 0.25), (ex + R, ey + a, z1 - 0.5), "eng_cowl", **ext)
    P.pair("eng_cowl_mid", (ex - b, ey - b, zc), (ex + b, ey + b, z1), "eng_cowl", **ext)
    P.pair("eng_cowl_tall", (ex - a, ey - R, zc + 0.5), (ex + a, ey + R, z1 - 0.25), "eng_cowl", **ext)
    cr, nr, pr = E["core_r"], E["nozzle_r"], E["plug_r"]
    P.pair("eng_core", (ex - cr, ey - cr, z1), (ex + cr, ey + cr, z2), "eng_hot", engine=(ex, ey))
    P.pair("eng_nozzle", (ex - nr, ey - nr, z2), (ex + nr, ey + nr, z3), "eng_hot", engine=(ex, ey))
    P.pair("eng_plug", (ex - pr, ey - pr, z3), (ex + pr, ey + pr, z4), "eng_hot", engine=(ex, ey))
    pw, py0, py1, pz0, pz1 = E["pylon"]
    P.pair("eng_pylon", (ex - pw, py0, pz0), (ex + pw, py1, pz1), "fairing")


def engine_painters(S):
    E = S.engine
    R = E["r"]
    z0 = E["z"][0]

    def rel(c):
        ex, ey = c.box.extra["engine"]
        sx = 1 if c.x > 0 else -1
        return (abs(c.x) - ex), c.y - ey, sx

    def lip(c):
        dx, dy, _ = rel(c)
        ri = E["lip_r"] - E["lip_t"]
        if c.front:
            return LIP if max(abs(dx), abs(dy)) > ri + 1 else LIP_LO
        inner = max(abs(dx), abs(dy)) < ri + 0.5
        if inner:
            return DUCT if c.z > z0 + 2 else LIP_LO
        if c.z < z0 + 3:
            return LIP if c.z < z0 + 2 else LIP_LO
        return cowl(c)

    def fan(c):
        dx, dy, _ = rel(c)
        r = math.hypot(dx, dy)
        ri = E["lip_r"] - E["lip_t"]
        if r < ri * 0.2:
            return SPINNER
        if r < ri * 0.32:
            return hexc("#8d939b")
        if r > ri * 1.02:
            return DUCT
        ang = math.degrees(math.atan2(dy, dx)) + r * 2.5
        return FAN_BLADE if ang % 30 < 12 else FAN

    def cowl(c):
        dy = rel(c)[1]
        if c.back:
            return DUCT
        if c.front:
            return Paint("roof", -0.1)
        k = 0.1 if dy > R * 0.75 else (0.04 if dy > R * 0.35 else (-0.16 if dy < -R * 0.75 else (-0.08 if dy < -R * 0.35 else 0.0)))
        if c.bottom:
            k = min(k, -0.14)
        # nothing changes along the cowl, so each face takes a sliver of the texture
        return Paint("roof", k)

    def hot(c):
        dx, dy, _ = rel(c)
        if c.back:
            return HOT_LO if max(abs(dx), abs(dy)) > 1.5 else FAN
        if c.top:
            return HOT_HI
        if c.bottom:
            return HOT_LO
        return HOT_HI if dy > E["core_r"] * 0.4 else (HOT_LO if dy < -E["core_r"] * 0.4 else HOT)

    return {"eng_lip": lip, "eng_fan": fan, "eng_cowl": cowl, "eng_hot": hot}


# ---------------------------------------------------------------- tail, tips, lamps


def paint_fin_lead(c):
    if c.side:
        q = c.loc[2] - c.box.frm[2]
        return Paint("door", 0.16 if q < 2 else 0.0)
    if c.face == "north":
        return Paint("door", 0.2)
    return Paint("door", 0.08 if c.face == "up" else -0.1)


def paint_fin_trail(c):
    if c.side:
        q = c.loc[2] - c.box.frm[2]
        n = c.box.to[2] - c.box.frm[2]
        return Paint("door", 0.1 if q > n - 1 else -0.07)
    return Paint("door", 0.05 if c.face == "up" else -0.14)


def paint_tip(c):
    """Wingtip devices, in the fin's colour; a cap's inboard part stays wing grey."""
    split = c.box.extra.get("grey")
    if split and abs(c.x) < split:
        return WING if c.top else (WING_LO if c.bottom else WING_EDGE)
    if c.top:
        return Paint("door", 0.1)
    if c.bottom:
        return Paint("door", -0.14)
    if c.face == "north":
        return Paint("door", 0.18)
    return Paint("door", 0.0)


def lamp_painter(colour):
    return lambda c: colour


def paint_nav(c):
    return LAMP_GREEN if c.x > 0 else LAMP_RED


def build_tail(P, S):
    T = S.tail
    def segs(le, te, cuts, top):
        """The stretches, the last one ending where the flush tip then reaches `top`."""
        end = flush_end(le, te, top)
        return [(a, b if k < len(cuts) - 1 else end, lo, hi) for k, (a, b, lo, hi) in enumerate(cuts)]

    lifting(P, "fin", "y", T["fin_le"], T["fin_te"], segs(T["fin_le"], T["fin_te"], T["fin_segs"], T["fin_top"]),
            "fin_lead", "fin_trail", thin=(0.75, 0.75), flush=True)
    lifting(P, "tailplane", "x", T["hs_le"], T["hs_te"], segs(T["hs_le"], T["hs_te"], T["hs_segs"], T["hs_tip"]),
            "wing_lead", "wing_trail", thin=(0.4, 0.5), flush=True, lead_from=T["hs_from"])
    hw, y0, z0, length, ang = T["fillet"]
    P.add("fin_fillet", (-hw, y0 - 4, z0), (hw, y0, z0 + length), "fin_fillet", origin=(0, y0, z0), rot=(-ang, 0, 0))
    for frm, to in T.get("antennas", ()):
        P.add("antenna", frm, to, "antenna")


def build_tips(P, S):
    for name, frm, to, kw in S.tips:
        P.pair(name, frm, to, "tip", **kw)
    R = getattr(S, "rake", None)
    if R:
        # a raked tip: the wing run on with a sharper sweep, stepping up
        end = flush_end(R["le"], R["te"], R["top"])
        segs = [(a, b if k < len(R["segs"]) - 1 else end, lo, hi) for k, (a, b, lo, hi) in enumerate(R["segs"])]
        lifting(P, "rake", "x", R["le"], R["te"], segs, "tip", "tip", flush=True)


def build_lamps(P, S):
    L = S.lamps
    for frm, to in L["nav"]:
        P.pair("lamp_nav", frm, to, "lamp_nav", lamp="glow")
    for frm, to in L["white"]:
        P.add("lamp_tail", frm, to, "lamp_white", lamp="glow")
    for frm, to in L["beacon"]:
        P.add("lamp_beacon", frm, to, "lamp_red", lamp="glow")
    for frm, to in L["landing"]:
        P.pair("lamp_landing", frm, to, "lamp_land", lamp="head")


# ---------------------------------------------------------------- landing gear


def wheel(P, name, bone, cx, cz, r, w, steps=3):
    """A wheel standing on the ground at (cx, cz): a stepped disc, each step set in a
    little so no two side faces lie in one plane."""
    shapes = ((1.0, 0.42), (0.84, 0.74), (0.53, 1.0)) if steps == 3 else ((1.0, 0.56), (0.6, 1.0))
    for k, (fy, fz) in enumerate(shapes):
        inset = 0.25 * k
        P.pair(f"{name}_{k}", (cx - w / 2 + inset, r - r * fy, cz - r * fz), (cx + w / 2 - inset, r + r * fy, cz + r * fz),
               "wheel", bone=bone, wheel=(cx, r, cz), radius=r)


def paint_wheel(c):
    _, wy, wz = c.box.extra["wheel"]
    r = c.box.extra["radius"]
    if c.side:
        q = math.hypot(c.z - wz, c.y - wy) / r
        if q < 0.2:
            return HUB_LO
        if q < 0.48:
            return HUB
        if q < 0.6:
            return HUB_LO
        return TIRE_SIDE if q < 0.82 else TIRE
    return TIRE


def paint_strut(c):
    oleo = c.box.extra.get("oleo")
    if oleo and oleo[0] <= c.y < oleo[1] and not (c.top or c.bottom):
        return OLEO
    if c.top:
        return STRUT
    if c.bottom:
        return STRUT_LO
    return STRUT if (c.side or c.front) else STRUT_LO


def paint_gear_door(c):
    """A gear door: the belly's colour outside, the dark well inside."""
    if c.side:
        if c.n[0] * c.x > 0:
            return Paint("lower", -0.06 if c.y > c.box.frm[1] + 1 else -0.2)
        return WELL
    return Paint("lower", -0.2)


def build_gear(P, S):
    G = S.gear
    zn = G["nose_z"]
    r, w, x = G["nose_wheel"]
    for k, cz in enumerate(G.get("nose_axles", (0.0,))):
        wheel(P, f"nose_wheel{k}", "gear_nose", x, zn + cz, r, w, steps=2)
    a = x + w / 2 + 0.5
    P.add("nose_axle", (-a, r - 1.25, zn - 1.25), (a, r + 1.25, zn + 1.25), "strut", bone="gear_nose")
    t = G["nose_strut"]
    P.add("nose_strut", (-t, r, zn - t), (t, G["nose_top"], zn + t), "strut", bone="gear_nose",
          oleo=(r + 1.25, r + 1.25 + G["nose_oleo"]))
    P.add("nose_light", (-t - 0.5, G["nose_light"], zn - t - 0.5), (t + 0.5, G["nose_light"] + 3, zn - t), "taxi_lens", bone="gear_nose")
    for frm, to in G["nose_doors"]:
        P.pair("nose_door", frm, to, "gear_door", bone="gear_nose")
    mx = G["main_x"]
    r, w, gap = G["main_wheel"]
    for k, cz in enumerate(G["main_axles"]):
        for side, sx in (("in", -1), ("out", 1)):
            wheel(P, f"main_wheel_{side}{k}", "gear_main", mx + sx * (gap + w) / 2, cz, r, w, steps=G.get("main_steps", 2))
        a = gap / 2 + w + 0.5
        P.pair("main_axle", (mx - a, r - 1.5, cz - 1.5), (mx + a, r + 1.5, cz + 1.5), "strut", bone="gear_main")
    if len(G["main_axles"]) > 1:
        z0, z1 = min(G["main_axles"]), max(G["main_axles"])
        P.pair("main_bogie", (mx - 2, r - 2, z0), (mx + 2, r + 2, z1), "strut", bone="gear_main")
    t = G["main_strut"]
    P.pair("main_strut", (mx - t, r, -t), (mx + t, G["main_top"], t), "strut", bone="gear_main",
           oleo=(r + 2, r + 2 + G["main_oleo"]))
    for frm, to in G["main_doors"]:
        P.pair("main_door", frm, to, "gear_door", bone="gear_main")
    for frm, to, kw in G.get("braces", ()):
        P.pair("main_brace", frm, to, "strut", bone="gear_main", **kw)


# ---------------------------------------------------------------- the models

PREVIEW = {
    "body": "minecraft:white_concrete",
    "lower": "minecraft:light_gray_concrete",
    "stripe": "minecraft:blue_concrete",
    "door": "minecraft:blue_concrete",
    "roof": "minecraft:white_concrete",
}


def sec(z0, z1, w, b, t, yc, segs=None, main=False, cabin=False):
    return dict(z0=z0, z1=z1, w=w, b=b, t=t, yc=yc, segs=segs, main=main, cabin=cabin)


def shifted(S, dz):
    """The airframe moved dz along z against its landing gear."""
    def box(b):
        (x0, y0, z0), (x1, y1, z1) = b
        return ((x0, y0, z0 + dz), (x1, y1, z1 + dz))

    def pts(p):
        return [(u, z + dz) for u, z in p]

    for s in S.sections:
        s["z0"] += dz
        s["z1"] += dz
    S.stripe_z = tuple(z + dz for z in S.stripe_z)
    S.radome_z += dz
    K = S.cockpit
    for key in ("z0", "z_rear", "block_z", "side_z", "post_z"):
        K[key] += dz
    W = S.wing
    W["le"] = (W["le"][0], W["le"][1] + dz, W["le"][2])
    W["te"] = pts(W["te"])
    W["fairing"] = [box(b) for b in W["fairing"]]
    E = S.engine
    E["z"] = tuple(z + dz for z in E["z"])
    E["pylon"] = E["pylon"][:3] + tuple(z + dz for z in E["pylon"][3:])
    T = S.tail
    for key in ("fin_le", "hs_le"):
        T[key] = (T[key][0], T[key][1] + dz, T[key][2])
    for key in ("fin_te", "hs_te"):
        T[key] = pts(T[key])
    f = T["fillet"]
    T["fillet"] = (f[0], f[1], f[2] + dz, f[3], f[4])
    T["antennas"] = [box(b) for b in T["antennas"]]
    R = getattr(S, "rake", None)
    if R:
        R["le"] = (R["le"][0], R["le"][1] + dz, R["le"][2])
        R["te"] = pts(R["te"])
    S.lamps = {k: [box(b) for b in v] for k, v in S.lamps.items()}
    S.eye = (S.eye[0], S.eye[1], S.eye[2] + dz)
    return S


def narrow_spec():
    S = SimpleNamespace(id="plane_narrow")
    S.yc, S.belly, S.stripe, S.win_rows = 61, 50, (64, 66), (68, 73)
    S.win_pitch, S.win_w = 8, 4
    S.door_rows, S.exit_rows, S.exit_at = (54, 84), (60, 75), (8,)
    S.stripe_z, S.radome_z = (-268, 252), -273
    S.sections = [
        sec(-283, -279, 6, 47, 58, 52.5),
        sec(-279, -274, 11, 43, 62, 53),
        sec(-274, -268, 16, 40, 66, 54),
        sec(-268, -260, 21, 37, 70, 55.5),
        sec(-260, -250, 25, 34, 73, 57),
        sec(-250, -238, 28, 31.5, 81, 59),
        sec(-238, -224, 30, 30, 88, 60.5),
        sec(-224, -210, 31.5, 28.5, 92.5, 61),
        sec(-210, 126, 32, 28, 94, 61, main=True,
            segs=[("door", 16)] + [("bay", 32)] * 4 + [("exit", 32)] + [("bay", 32)] * 5),
        sec(126, 158, 32, 31.5, 94, 61, segs=[("bay", 32)], cabin=True),
        sec(158, 190, 32, 35, 94, 61, segs=[("bay", 16), ("door", 16)], cabin=True),
        sec(190, 222, 27, 43.5, 93, 67),
        sec(222, 252, 22, 52, 92, 71),
        sec(252, 278, 16, 60, 90, 74.5),
        sec(278, 300, 11, 67.5, 87, 77),
        sec(300, 312, 7, 72, 84, 78),
        sec(312, 318, 4, 75, 81, 78),
    ]
    S.cockpit = dict(
        z0=-264, y0=72, slope=0.7, hw=14.5, step=2, y_top=88, brow_top=88.75, z_rear=-222,
        block_z=-249, block_hw=23.5, block_top=82.5, top_hw=17.5, glass=(72, 82),
        side_z=-231, post_z=-241,
    )
    t27, t17 = math.tan(math.radians(27)), math.tan(math.radians(17))
    S.wing = dict(
        le=(0, -83, 27), te=[(0, 33), (105, 33), (283, 33 + 178 * t17)],
        segs=[(30, 105, 38, 50), (105, 200, 45, 54), (200, 283, 52, 58)],
        tracks=(110, 163, 218), track_size=(2, 4.5, 26),
        fairing=[
            ((-35.5, 26, -76), (35.5, 51, 46)), ((-25, 24, -70), (25, 26, 40)),
            ((-33.5, 27.5, -90), (33.5, 47, -76)), ((-33.5, 27.5, 46), (33.5, 47, 62)),
        ],
    )
    S.engine = dict(
        x=92, y=28.5, r=18.5, oct=(8, 13), z=(-98, -40, -22, -12, -4),
        lip_r=15.5, lip_t=4, lip_d=8, lip_c=6.5, lip_k=12.5, core_r=11, nozzle_r=7.5, plug_r=3.5, seam=30,
        pylon=(2.5, 38.5, 49.5, -86, -14),
    )
    t10, t33, t12 = (math.tan(math.radians(a)) for a in (10, 33, 12))
    S.tail = dict(
        fin_le=(86, 197, 40), fin_te=[(86, 293), (186, 293 + 100 * t10)], fin_top=188.8,
        fin_segs=[(86, 153, -2.5, 2.5), (153, 0, -2.5, 2.5)],
        fillet=(2, 93.5, 190, 40, 17), antennas=[((-0.5, 93.5, -150), (0.5, 98, -144))],
        hs_le=(0, 236 - 12 * t33, 33), hs_te=[(0, 293.4), (100, 293.4 + 100 * t12)], hs_tip=99.6, hs_from=16,
        hs_segs=[(0, 64, 70.5, 76), (64, 0, 73, 77)],
    )
    tz = S.wing["le"][1] + 283 * t27
    S.tips = [
        ("wingtip", (275.5, 51.5, tz + 1), (286.4, 58.5, tz + 27), dict(grey=281)),
        ("winglet", (283.4, 56, tz + 6), (286.15, 86, tz + 22), dict(origin=(0, 56, tz + 6), rot=(32, 0, 0))),
    ]
    S.lamps = dict(
        nav=[((283.9, 53.5, tz + 0.25), (286.65, 56.5, tz + 3))],
        white=[((-1.5, 76.5, 318), (1.5, 79.5, 318.5))],
        beacon=[((-1.5, 94, -30), (1.5, 95.5, -27)), ((-1.5, 22.75, -20), (1.5, 24, -17))],
        landing=[((28.5, 40, -90.5), (32.5, 43, -90))],
    )
    zn = -201.6
    S.gear = dict(
        nose_z=zn, nose_wheel=(6, 4, 5), nose_strut=1.5, nose_top=30, nose_oleo=7, nose_light=17,
        nose_doors=[((7.25, 19.5, zn - 14), (8, 29.5, zn + 9))],
        main_x=60.8, main_wheel=(9.5, 6, 7.6), main_axles=(0.0,), main_strut=2, main_top=40, main_oleo=8,
        main_doors=[((63, 22, -7), (63.75, 38.5, 7))],
        braces=[((60.8 - 28, 19, -1), (60.8, 21.5, 1), dict(origin=(60.8, 20.25, 0), rot=(0, 0, -40)))],
    )
    S.eye = (0.0, 72.0, -112.0)
    return S


def build(S):
    P = Parts()
    lay = build_fuselage(P, S)
    main = next(i for i, s in enumerate(S.sections) if s.get("main"))
    painters = fuselage_painters(S, lay[main])
    build_cockpit(P, S)
    painters.update(cockpit_painters(S, painters["skin"]))
    build_wing(P, S)
    build_engines(P, S)
    painters.update(engine_painters(S))
    build_tail(P, S)
    build_tips(P, S)
    build_lamps(P, S)
    build_gear(P, S)
    painters.update({
        "wing_lead": paint_lead, "wing_trail": paint_trail, "fairing": paint_fairing,
        "fin_lead": paint_fin_lead, "fin_trail": paint_fin_trail, "fin_fillet": paint_tip, "tip": paint_tip,
        "lamp_nav": paint_nav, "lamp_white": lamp_painter(LAMP_WHITE), "lamp_red": lamp_painter(LAMP_RED),
        "lamp_land": lamp_painter(LAMP_LAND), "taxi_lens": lamp_painter(LAMP_LAND),
        "wheel": paint_wheel, "strut": paint_strut, "gear_door": paint_gear_door,
        "antenna": lambda c: FAIRING_HI if c.top else FAIRING,
    })
    return Model(S.id, P.boxes, painters, PREVIEW, eye=S.eye)


def wide_spec():
    S = SimpleNamespace(id="plane_wide")
    S.yc, S.belly, S.stripe, S.win_rows = 83.5, 68, (85, 88), (90, 96)
    S.win_pitch, S.win_w = 8, 4
    S.door_rows, S.exit_rows, S.exit_at = (76, 107), (0, 0), ()
    S.stripe_z, S.radome_z = (-518, 420), -521
    bays = lambda n: [("bay", 40)] * n
    S.sections = [
        sec(-541, -536, 7, 69.5, 80, 74.5),
        sec(-536, -528, 14, 64, 86, 75),
        sec(-528, -518, 21, 58, 91, 76),
        sec(-518, -506, 27, 52.5, 97, 77),
        sec(-506, -492, 33, 47.5, 101.5, 79),
        sec(-492, -476, 38, 43.5, 111.5, 81),
        sec(-476, -457, 42, 40, 122, 82),
        sec(-457, -437, 45, 37, 129, 83),
        sec(-437, -421, 46.5, 36, 131, 83.5),
        sec(-421, 159, 47, 35, 132, 83.5, main=True,
            segs=[("door", 20)] + bays(4) + [("door", 20)] + bays(7) + [("door", 20)] + bays(2)),
        sec(159, 199, 47, 39, 132, 83.5, segs=bays(1), cabin=True),
        sec(199, 239, 47, 45, 132, 83.5, segs=bays(1), cabin=True),
        sec(239, 299, 47, 53, 132, 83.5, segs=bays(1) + [("door", 20)], cabin=True),
        sec(299, 340, 42, 62, 131, 96),
        sec(340, 382, 35, 73, 130, 101),
        sec(382, 420, 27, 83, 128, 105),
        sec(420, 452, 19, 93, 126, 109),
        sec(452, 479, 12, 100, 123, 111),
        sec(479, 499, 6, 106, 119, 112.5),
    ]
    S.cockpit = dict(
        z0=-511, y0=98, slope=0.75, hw=20.5, step=3, y_top=122, brow_top=123.5, z_rear=-455, thick=3, drop=14,
        block_z=-494, block_hw=31.5, block_top=113.5, top_hw=26, glass=(99, 113),
        side_z=-467, post_z=-481,
    )
    t34 = math.tan(math.radians(34))
    le0 = -171.7
    te_tip = le0 + 470 * t34 + 44
    S.wing = dict(
        le=(0, le0, 34), te=[(0, 52), (172, 52), (470, te_tip)],
        segs=[(44, 105, 46, 66), (105, 172, 52, 70), (172, 270, 58, 73), (270, 380, 67, 78), (380, 462, 76, 83)],
        tracks=(190, 262, 334, 406), track_size=(3, 7, 40),
        fairing=[
            ((-51, 31, -162), (51, 67, 72)), ((-36, 28.5, -150), (36, 31, 60)),
            ((-49, 34.5, -186), (49, 61, -162)), ((-49, 34.5, 72), (49, 61, 102)),
        ],
    )
    S.engine = dict(
        x=166, y=40, r=29, oct=(12.5, 20.5), z=(-134, -60, -32, -18, -6),
        lip_r=24.5, lip_t=6, lip_d=12, lip_c=10.5, lip_k=20, core_r=17, nozzle_r=12, plug_r=5.5, seam=46,
        pylon=(4, 56, 69.75, -114, -20),
    )
    S.tail = dict(
        fin_le=(124, 300, 40), fin_te=[(124, 440), (264, 440 + 140 * 0.25)], fin_top=272,
        fin_segs=[(124, 217, -4, 4), (217, 0, -4, 4)],
        fillet=(3, 131.5, 301, 62, 17), antennas=[((-0.75, 131.5, -260), (0.75, 138, -252)), ((-0.75, 131.5, 60), (0.75, 137, 67))],
        hs_le=(0, 345, 35), hs_te=[(0, 457), (152, 457 + 152 * 0.24)], hs_tip=152, hs_from=29,
        hs_segs=[(0, 95, 100.5, 108), (95, 0, 103, 109)],
    )
    rx = 458
    rz = le0 + rx * t34
    rte = interp(S.wing["te"], rx)[0]
    S.tips = []
    S.rake = dict(
        le=(rx, rz, 45), te=[(rx, rte), (rx + 46, rte + 46 * (te_tip - 52) / 298)], top=504,
        segs=[(rx, 482, 80, 85.5), (482, 0, 84, 88.5)],
    )
    tzl = rz + 46
    S.lamps = dict(
        nav=[((500.5, 85, tzl - 4), (504.25, 87.5, tzl + 1.5))],
        white=[((-2, 110.5, 499), (2, 114.5, 499.5))],
        beacon=[((-2, 132, -60), (2, 134, -56)), ((-2, 26.75, -40), (2, 28.5, -36))],
        landing=[((42, 52, -186.5), (47, 56, -186))],
    )
    zn = -459.2
    S.gear = dict(
        nose_z=zn, nose_wheel=(8.5, 5.5, 7), nose_strut=2.25, nose_top=42, nose_oleo=10, nose_light=24,
        nose_doors=[((10, 29, zn - 15), (11, 41.5, zn + 10))],
        main_x=84.8, main_wheel=(11.5, 7.5, 9), main_axles=(-13.0, 13.0), main_strut=3, main_top=48, main_oleo=11,
        main_doors=[((88.5, 30, -10), (89.5, 47, 10))],
        braces=[((84.8 - 37, 24.5, -1.5), (84.8, 28, 1.5), dict(origin=(84.8, 26.25, 0), rot=(0, 0, -40)))],
    )
    S.eye = (0.0, 93.0, -200.0)
    # the nose 33.5 m ahead of the main wheels, the tail 31.5 m behind
    return shifted(S, 5.0)


def build_narrow():
    return build(narrow_spec())


def build_wide():
    return build(wide_spec())


MODELS = [build_narrow, build_wide]
