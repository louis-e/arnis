#!/usr/bin/env python3
"""Builds gateway_arch.schem, the Gateway Arch in St. Louis, beside this script.

    python3 assets/structures/landmarks/build_gateway_arch.py [--out PATH]

One block per metre, x east, z south. Layer GROUND sits on the plaza at the leg feet; the legs go
on GROUND layers below it. Deps: numpy.

Shape: the design equation of the centroid curve from the construction drawings (as quoted by
the NPS and en.wikipedia "Gateway Arch"), y = fc - A(cosh(Cx/L) - 1) ft with fc 625.0925,
Qb 1262.6651, Qt 125.1406, L 299.2239, A = fc/(Qb/Qt - 1), C = arcosh(Qb/Qt). Each section,
normal to the curve, is an equilateral triangle of area Qt cosh(Cx/L), falling linearly with
height from 54 ft sides at the feet to 17 ft at the top, flat face outward: the outer faces then
stand 630 ft apart at the plaza and the top is 630 ft up, as published. Position, turn and plaza
level come from fitting that solid to USGS 3DEP lidar (USGS_LPC_MO_StLouis_2017, public domain):
2216 returns off the arch sit a median 1.8 cm from its
surface (21 cm if the sides tapered linearly instead), with the arch plane 17.77 deg east of north
and the centre 0.7 m north of the mapped node.
"""
import gzip
import io
import math
import os
import re
import struct
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "gateway_arch.schem")
SCHEMATIC_RS = os.path.join(HERE, "..", "..", "..", "src", "structures", "schematic.rs")
DATA_VERSION = 4438

FT = 0.3048
FC, QB, QT, LH = 625.0925, 1262.6651, 125.1406, 299.2239
C = math.acosh(QB / QT)
A = FC / (QB / QT - 1)
K = C / LH

# Fitted to the lidar: arch plane azimuth, and the centre that the anchor pins.
AZIMUTH = 17.77
LAT, LON = 38.624_608_0, -90.184_975_7

GROUND = 3
HALF_X, HALF_Z = 37, 95
W, L = 2 * HALF_X + 1, 2 * HALF_Z + 1
AX, AZ = HALF_X + 0.5, HALF_Z + 0.5
TOP = GROUND + 193

SKIN = "iron_block"
SLAB = "polished_diorite_slab"
WINDOWS, WINDOW_STEP = 10, 2.0
WINDOW = "black_stained_glass"
LIGHT = "light[level=15,waterlogged=false]"
LIGHT_SPACING = 9.0


def section(x):
    """Centroid height, unit tangent and outward normal, and triangle side, at x feet."""
    h = FC - A * (np.cosh(K * x) - 1)
    dh = -A * K * np.sinh(K * x)
    n = np.hypot(1.0, dh)
    s = np.sqrt(4 * QT * np.cosh(K * x) / math.sqrt(3))
    return h, (1 / n, dh / n), (-dh / n, 1 / n), s


def locate(u, w):
    """Parameter (feet) of the section whose normal plane holds (u, w); u >= 0, feet."""
    lo = np.zeros_like(u)
    hi = np.full_like(u, 1.3 * LH)
    for _ in range(32):
        mid = (lo + hi) / 2
        h, t, _, _ = section(mid)
        f = (u - mid) * t[0] + (w - h) * t[1]
        lo = np.where(f > 0, mid, lo)
        hi = np.where(f > 0, hi, mid)
    return (lo + hi) / 2


def arch_coords(ex, sz, up):
    """Model offsets from the anchor (east, south, up, metres) to arch-plane feet."""
    a = math.radians(AZIMUTH)
    north = -sz
    u = (ex * math.sin(a) + north * math.cos(a)) / FT
    v = (ex * math.cos(a) - north * math.sin(a)) / FT
    return u, v, up / FT


def solid_depth(ex, sz, up):
    """How far inside the arch each point is, metres (negative outside)."""
    u, v, w = arch_coords(ex, sz, up)
    au = np.abs(u)
    x = locate(au, w)
    h, _, nrm, s = section(x)
    n = (au - x) * nrm[0] + (w - h) * nrm[1]
    d_out = n - s / (2 * math.sqrt(3))
    d_face = (math.sqrt(3) * np.abs(v) - n - s / math.sqrt(3)) / 2
    return -np.maximum(d_out, d_face) * FT


def voxel_centres():
    ix, iy, iz = np.meshgrid(np.arange(W), np.arange(TOP + 1), np.arange(L), indexing="ij")
    return ix, iy, iz


def build():
    ix, iy, iz = voxel_centres()
    ex = ix + 0.5 - AX
    sz = iz + 0.5 - AZ
    up = iy + 0.5 - GROUND
    depth = solid_depth(ex.ravel(), sz.ravel(), up.ravel()).reshape(ix.shape)
    # Half-block occupancy where the surface passes: 4x4 columns by 2 heights per half.
    lower = depth > 0
    upper = depth > 0
    band = np.abs(depth) < 1.0
    bi = np.nonzero(band)
    sub = (np.arange(4) + 0.5) / 4 - 0.5
    for half, out in ((0, lower), (1, upper)):
        acc = np.zeros(len(bi[0]))
        for dx in sub:
            for dz in sub:
                for dy in (0.125, 0.375):
                    d = solid_depth(
                        ix[bi] + 0.5 + dx - AX,
                        iz[bi] + 0.5 + dz - AZ,
                        iy[bi] + half * 0.5 + dy - GROUND,
                    )
                    acc += d > 0
        out[bi] = acc >= 16
    full = lower & upper
    solid = lower | upper

    # Hollow inside a 26-connected skin, as the real legs are, above the plaza.
    core = full.copy()
    for dx in (-1, 0, 1):
        for dy in (-1, 0, 1):
            for dz in (-1, 0, 1):
                core &= np.roll(np.roll(np.roll(full, dx, 0), dy, 1), dz, 2)
    core[:, : GROUND + 2, :] = False

    shell = solid & ~core
    outside = ~solid
    exposed = np.zeros_like(solid)
    for ax in range(3):
        for s in (1, -1):
            exposed |= np.roll(outside, s, axis=ax)
    # 16 small windows a side light the deck: one block every WINDOW_STEP m on each inner face.
    skin = np.argwhere(shell & full & exposed)
    centres = skin + 0.5 - np.array([AX, GROUND, AZ])
    a = math.radians(AZIMUTH)
    windows = set()
    for side in (1, -1):
        for k in range(WINDOWS):
            um = (k - (WINDOWS - 1) / 2) * WINDOW_STEP
            h, _, nrm, s = section(np.array(um / FT))
            n0 = -0.2 * s
            v0 = side * (n0 + s / math.sqrt(3)) / math.sqrt(3)
            uu, ww = um / FT + nrm[0] * n0, h + nrm[1] * n0
            p = np.array([
                (uu * math.sin(a) + v0 * math.cos(a)) * FT,
                ww * FT,
                (-uu * math.cos(a) + v0 * math.sin(a)) * FT,
            ])
            windows.add(tuple(skin[np.argmin(((centres - p) ** 2).sum(1))]))

    blocks = floodlight(shell, outside, exposed)
    for p in zip(*np.nonzero(shell)):
        if p in windows:
            blocks[p] = WINDOW
        elif full[p]:
            blocks[p] = SKIN
        elif lower[p]:
            blocks[p] = f"{SLAB}[type=bottom,waterlogged=false]"
        else:
            blocks[p] = f"{SLAB}[type=top,waterlogged=false]"
    return blocks


def floodlight(shell, outside, exposed):
    """Night floodlighting as invisible light off the skin; low down only over the arch's own
    columns so nothing on the ground is cleared, under the span only from 30 m up."""
    top = np.nonzero(shell.any(axis=(0, 2)))[0].max()
    low = np.where(shell.any(axis=1), shell.argmax(axis=1), shell.shape[1])
    cand = set()
    for x, y, z in np.argwhere(shell & exposed):
        for dx, dy, dz in ((1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1), (0, 1, 0), (0, -1, 0)):
            c = (x + dx, y + dy, z + dz)
            if 0 <= c[0] < W and 0 <= c[2] < L and GROUND <= c[1] < top \
                    and outside[c] and (low[c[0], c[2]] < c[1] or c[1] >= GROUND + 30):
                cand.add(c)
    chosen = []
    for c in sorted(cand, key=lambda c: (c[1], c[0], c[2])):
        # closer at the feet, where the floodlights stand
        gap = LIGHT_SPACING * (0.75 if c[1] < GROUND + 30 else 1.0)
        if all((c[0] - q[0]) ** 2 + (c[1] - q[1]) ** 2 + (c[2] - q[2]) ** 2
               >= gap ** 2 for q in chosen):
            chosen.append(c)
    return {c: LIGHT for c in chosen}


# ---------------------------------------------------------------- Sponge v3


def _payload(out, tag, value):
    if tag == 2:
        out.write(struct.pack(">h", value))
    elif tag == 3:
        out.write(struct.pack(">i", value))
    elif tag == 7:
        out.write(struct.pack(">i", len(value)) + bytes(value))
    elif tag == 8:
        raw = value.encode()
        out.write(struct.pack(">H", len(raw)) + raw)
    elif tag == 9:
        kind, items = value
        out.write(bytes([kind]) + struct.pack(">i", len(items)))
        for item in items:
            _payload(out, kind, item)
    elif tag == 10:
        for key, (t, v) in value.items():
            raw = key.encode()
            out.write(bytes([t]) + struct.pack(">H", len(raw)) + raw)
            _payload(out, t, v)
        out.write(b"\x00")
    elif tag == 11:
        out.write(struct.pack(">i", len(value)) + struct.pack(f">{len(value)}i", *value))
    else:
        raise ValueError(tag)


def sponge(blocks, size):
    w, h, l = size
    palette = {"minecraft:air": 0}
    ids = np.zeros((h, l, w), np.int64)
    for (x, y, z), name in blocks.items():
        ids[y, z, x] = palette.setdefault("minecraft:" + name, len(palette))
    data = bytearray()
    for v in ids.ravel():
        v = int(v)
        while True:
            b = v & 0x7F
            v >>= 7
            if v:
                data.append(b | 0x80)
            else:
                data.append(b)
                break
    schematic = {
        "Version": (3, 3),
        "DataVersion": (3, DATA_VERSION),
        "Width": (2, w),
        "Height": (2, h),
        "Length": (2, l),
        "Offset": (11, [0, 0, 0]),
        "Blocks": (10, {
            "Palette": (10, {k: (3, v) for k, v in palette.items()}),
            "Data": (7, data),
            "BlockEntities": (9, (10, [])),
        }),
    }
    out = io.BytesIO()
    out.write(b"\x0a" + struct.pack(">H", 0))
    _payload(out, 10, {"Schematic": (10, schematic)})
    return gzip.compress(out.getvalue(), mtime=0), palette


def unmapped(palette):
    """Block names the Arnis schematic loader would drop."""
    try:
        src = open(SCHEMATIC_RS).read()
    except OSError:
        return []
    known = set(re.findall(r'"([a-z_]+)"', src))
    names = {k.split("[")[0].removeprefix("minecraft:") for k in palette}
    return sorted(n for n in names if n != "air" and n not in known)


def main():
    out = sys.argv[sys.argv.index("--out") + 1] if "--out" in sys.argv else OUT
    blocks = build()
    data, palette = sponge(blocks, (W, TOP + 1, L))
    with open(out, "wb") as f:
        f.write(data)
    top = max(p[1] for p in blocks) + 1
    print(f"{out}: {W}x{TOP + 1}x{L}, {len(blocks)} blocks, {len(palette)} states, "
          f"top {top - GROUND} m above the plaza")
    missing = unmapped(palette)
    if missing:
        print("not in map_structure_block:", ", ".join(missing))


if __name__ == "__main__":
    main()
