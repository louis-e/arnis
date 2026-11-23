#!/usr/bin/env python3
"""Builds christ_the_redeemer.schem beside this script: Cristo Redentor on the Corcovado summit.

    python3 build_christ_the_redeemer.py [CACHE_DIR]

Needs numpy and pillow. Downloads go to CACHE_DIR (default $ARNIS_CACHE or ~/.cache/arnis-landmarks):
- the statue mesh, louis-e, CC BY 4.0: https://arnismc.com/assets/3dmodels/christ_the_redeemer.stl
- one front photo for the statue's silhouette: "Christ the Redeemer - Cristo Redentor.jpg",
  Arne Müseler, Wikimedia Commons, CC BY-SA 3.0 DE; only its outline is measured.

Sizes: statue 30 m, pedestal 8 m, facing 80.9 deg (the mapped pedestal octagon, which the
orthophotos' arms agree with). Terrace, stairs and escalator from the mapped pedestrian area and
ways (OSM) and IPP's 2019/2024 true orthophotos of Rio; levels from IPP's 2019 LiDAR terrain
model. Below the terraces a skirt of summit rock falls 2.5:1 on the cliff sides. Axes: x east,
y up, z south, 1 block per metre. The monument node sits mid-block at (OX, OZ); the platform
floor is layer L0; layer 0 is the footing Arnis carries down to the ground.
"""
import gzip
import math
import os
import struct
import sys
import urllib.request

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "christ_the_redeemer.schem")
STL_URL = "https://arnismc.com/assets/3dmodels/christ_the_redeemer.stl"
PHOTO_URL = ("https://upload.wikimedia.org/wikipedia/commons/thumb/4/4f/"
             "Christ_the_Redeemer_-_Cristo_Redentor.jpg/1280px-Christ_the_Redeemer_-_Cristo_Redentor.jpg")

FACING = 80.9
STATUE_H = 30.0
PEDESTAL_H = 8
# the pedestal octagon's middle, 0.47 m west of the monument node, rounds onto its block
PED = (0, 0)
SKIRT = 35
SKIRT_SLOPE = 2.5
# the walkway's ridge east of here is missing from the DEM, so the model ends at its junction
WALK_END = 16.5
L0 = SKIRT + 3
OX, OZ = 21, 24
WIDTH, LENGTH = 52, 54
HEIGHT = L0 + PEDESTAL_H + int(STATUE_H) + 1


def fetch(url, cache, name):
    os.makedirs(cache, exist_ok=True)
    path = os.path.join(cache, name)
    if not os.path.exists(path):
        req = urllib.request.Request(url, headers={"User-Agent": "arnis-landmark-builder"})
        with urllib.request.urlopen(req, timeout=120) as r, open(path, "wb") as f:
            f.write(r.read())
    return path


# ---------------------------------------------------------------- grid


class Grid:
    def __init__(self, w, h, l):
        self.ids = np.zeros((w, h, l), np.int32)
        self.names = ["minecraft:air"]
        self.index = {"minecraft:air": 0}

    def pid(self, name):
        if not name.startswith("minecraft:"):
            name = "minecraft:" + name
        if name not in self.index:
            self.index[name] = len(self.names)
            self.names.append(name)
        return self.index[name]

    def inside(self, x, y, z):
        w, h, l = self.ids.shape
        return 0 <= x < w and 0 <= y < h and 0 <= z < l

    def set(self, x, y, z, name):
        if self.inside(x, y, z):
            self.ids[x, y, z] = self.pid(name)

    def get(self, x, y, z):
        return self.names[self.ids[x, y, z]] if self.inside(x, y, z) else "minecraft:air"

    def empty(self, x, y, z):
        return self.inside(x, y, z) and self.ids[x, y, z] == 0

    def save(self, path):
        w, h, l = self.ids.shape
        used = sorted(set(np.unique(self.ids).tolist()) | {0})
        remap = np.zeros(len(self.names), np.int32)
        for i, u in enumerate(used):
            remap[u] = i
        data = remap[self.ids].transpose(1, 2, 0).reshape(-1)
        blob = bytearray()
        for v in data.tolist():
            while v > 0x7F:
                blob.append((v & 0x7F) | 0x80)
                v >>= 7
            blob.append(v)

        def tag(t, name):
            raw = name.encode()
            return struct.pack(">bH", t, len(raw)) + raw

        body = bytearray()
        body += tag(3, "Version") + struct.pack(">i", 2)
        body += tag(3, "DataVersion") + struct.pack(">i", 4438)
        body += tag(2, "Width") + struct.pack(">h", w)
        body += tag(2, "Height") + struct.pack(">h", h)
        body += tag(2, "Length") + struct.pack(">h", l)
        body += tag(3, "PaletteMax") + struct.pack(">i", len(used))
        body += tag(10, "Palette")
        for i, u in enumerate(used):
            body += tag(3, self.names[u]) + struct.pack(">i", i)
        body += b"\x00"
        body += tag(7, "BlockData") + struct.pack(">i", len(blob)) + bytes(blob)
        with open(path, "wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as f:
            f.write(tag(10, "Schematic") + bytes(body) + b"\x00")
        return len(used)


# ---------------------------------------------------------------- statue


def read_stl(path):
    d = open(path, "rb").read()
    n = struct.unpack("<I", d[80:84])[0]
    rec = np.dtype([("n", "<f4", 3), ("v", "<f4", (3, 3)), ("a", "<u2")])
    return np.frombuffer(d[84:84 + 50 * n], dtype=rec)["v"].astype(np.float64)


def voxelize(tris, lo, shape, sub):
    """Solid fine occupancy by ray parity along y; `sub` samples per block edge."""
    nx, ny, nz = (s * sub for s in shape)
    step = 1.0 / sub
    xs = lo[0] + (np.arange(nx) + 0.5) * step
    zs = lo[2] + (np.arange(nz) + 0.5) * step
    hits = [[] for _ in range(nx * nz)]
    for p0, p1, p2 in tris:
        x0, x1 = min(p0[0], p1[0], p2[0]), max(p0[0], p1[0], p2[0])
        z0, z1 = min(p0[2], p1[2], p2[2]), max(p0[2], p1[2], p2[2])
        i0 = max(0, int(math.ceil((x0 - lo[0]) / step - 0.5)))
        i1 = min(nx - 1, int(math.floor((x1 - lo[0]) / step - 0.5)))
        k0 = max(0, int(math.ceil((z0 - lo[2]) / step - 0.5)))
        k1 = min(nz - 1, int(math.floor((z1 - lo[2]) / step - 0.5)))
        if i0 > i1 or k0 > k1:
            continue
        d = (p1[0] - p0[0]) * (p2[2] - p0[2]) - (p2[0] - p0[0]) * (p1[2] - p0[2])
        if abs(d) < 1e-12:
            continue
        gx, gz = np.meshgrid(xs[i0:i1 + 1], zs[k0:k1 + 1], indexing="ij")
        u = ((gx - p0[0]) * (p2[2] - p0[2]) - (p2[0] - p0[0]) * (gz - p0[2])) / d
        v = ((p1[0] - p0[0]) * (gz - p0[2]) - (gx - p0[0]) * (p1[2] - p0[2])) / d
        inside = (u >= 0) & (v >= 0) & (u + v <= 1)
        y = p0[1] + u * (p1[1] - p0[1]) + v * (p2[1] - p0[1])
        for a, b in zip(*np.nonzero(inside)):
            hits[(i0 + a) * nz + (k0 + b)].append(y[a, b])
    ys = lo[1] + (np.arange(ny) + 0.5) * step
    occ = np.zeros((nx, ny, nz), bool)
    for idx, h in enumerate(hits):
        if len(h) < 2:
            continue
        h = np.sort(np.array(h))
        h = h[np.concatenate([[True], np.diff(h) > 1e-7])]
        i, k = divmod(idx, nz)
        for s in range(0, len(h) - 1, 2):
            occ[i, :, k] |= (ys >= h[s]) & (ys < h[s + 1])
    return occ


def photo_silhouette(path):
    """Statue pixels of the front photo: bright, grey, grown from the robe into its shaded rims."""
    from PIL import Image, ImageFilter

    def grow(seed, mask, steps=10 ** 6):
        comp = seed & mask
        for _ in range(steps):
            d = comp.copy()
            d[1:] |= comp[:-1]; d[:-1] |= comp[1:]; d[:, 1:] |= comp[:, :-1]; d[:, :-1] |= comp[:, 1:]
            d &= mask
            if d.sum() == comp.sum():
                break
            comp = d
        return comp

    def morph(m, op, size):
        im = Image.fromarray((m * 255).astype(np.uint8))
        for f in op:
            im = im.filter(ImageFilter.MaxFilter(size) if f == "+" else ImageFilter.MinFilter(size))
        return np.asarray(im) > 127

    a = np.asarray(Image.open(path).convert("RGB")).astype(float)
    mx, mn = a.max(-1), a.min(-1)
    sat = (mx - mn) / (mx + 1)
    roi = np.zeros(mx.shape, bool)
    roi[220:1342, 40:1240] = True
    strong = morph(morph((mx > 150) & (sat < 0.25) & roi, "", 1), "+-", 7)
    seed = np.zeros_like(strong)
    seed[900, 660] = True
    core = grow(seed, strong)
    weak = (mx > 70) & (sat < 0.24) & (a[..., 1] <= a[..., 0] + 6) & roi
    sil = morph(core | grow(core, weak | core, 30), "+-", 5)
    bg = ~sil
    edge = np.zeros_like(bg)
    edge[0], edge[-1], edge[:, 0], edge[:, -1] = bg[0], bg[-1], bg[:, 0], bg[:, -1]
    sil = ~grow(edge, bg)
    for y in range(236, 430):
        xs = np.nonzero(sil[y, 560:760])[0]
        if len(xs):
            sil[y, 560 + xs.min():560 + xs.max() + 1] = True
    sil[1290:, :540] = False
    return morph(sil, "-+", 9)


def photo_cover(sil, lo_z, L, H, sub):
    """Silhouette share of each 1/sub m cell of the front view (rows y up, columns design z)."""
    cx, top, toe = 650.0, 237.0, 1340.0
    k = STATUE_H / (toe - top)
    ys, xs = np.nonzero(sil)
    zi = np.floor((-(xs - cx) * k - lo_z) * sub).astype(int)
    yi = np.floor((toe - ys) * k * sub).astype(int)
    ok = (zi >= 0) & (zi < L * sub) & (yi >= 0) & (yi < H * sub)
    area = np.zeros((H * sub, L * sub))
    np.add.at(area, (yi[ok], zi[ok]), 1)
    return area * (k * sub) ** 2


def statue_blocks(stl, photo):
    """The statue in the design frame (facing +x, right hand +z), as {(dx, y, dz): block} around
    the pedestal centre; y from the pedestal top."""
    t = read_stl(stl)
    low = t[..., 2].min(axis=1)
    ped, body = t[low <= 935], t[low > 935]
    pc = [(ped[..., 0].min() + ped[..., 0].max()) / 2, (ped[..., 1].min() + ped[..., 1].max()) / 2]
    s = STATUE_H / (body[..., 2].max() - 937.0)
    # STL front is -y; a proper rotation, not the y/z swap, keeps the drapery the right way round
    tris = np.stack([-(body[..., 1] - pc[1]) * s, (body[..., 2] - 937.0) * s,
                     -(body[..., 0] - pc[0]) * s - 0.08], -1)
    W, H, L = 9, int(STATUE_H), 33
    lo = np.array([-4.5, 0.0, -16.5])
    sub = 4
    fine = voxelize(tris, lo, (W, H, L), sub)
    cov = fine.reshape(W, sub, H, sub, L, sub).mean(axis=(1, 3, 5))
    solid = cov >= 0.45
    head = np.zeros_like(solid)
    head[:, 25:, :] = np.abs(np.arange(L) + lo[2] + 0.5)[None, None, :] < 2.5
    solid |= head & (cov >= 0.3)
    # keep what the front, side and top views show of thin drapery
    for ax in range(3):
        dims = [W, H, L]
        del dims[ax]
        area = fine.any(axis=ax).reshape(dims[0], sub, dims[1], sub).mean(axis=(1, 3))
        cm, sm = np.moveaxis(cov, ax, -1), np.moveaxis(solid, ax, -1)
        for a, b in zip(*np.nonzero(area >= 0.5)):
            if not sm[a, b].any():
                sm[a, b, int(cm[a, b].argmax())] = True
    # arms and hands take their height and thickness from the photo: the mesh droops them
    sil = photo_silhouette(photo)
    pa2 = photo_cover(sil, lo[2] + 0.08, L, H, 2)
    slabs = {}
    for k in range(L):
        zc = k + lo[2] + 0.5
        # arms from the shoulder line out, and the shoulders beside the head; the head keeps the mesh
        if abs(zc) < 2.5:
            continue
        body = abs(zc) < 4.5
        y_from = 24 if body else 17
        col = pa2[:, 2 * k:2 * k + 2].mean(axis=1)
        rows = np.nonzero(col >= 0.5)[0]
        rows = rows[rows >= 2 * y_from]
        # one unbroken run: the one rising from the shoulder, or out on the arm the longest
        runs = np.split(rows, np.nonzero(np.diff(rows) > 1)[0] + 1) if len(rows) else []
        if runs:
            rows = runs[0] if body else max(runs, key=len)
        xs = np.nonzero(solid[:, y_from:, k].any(axis=1))[0]
        if len(xs) == 0:
            xs = np.nonzero(solid[:, y_from - 1, k])[0]
        solid[:, y_from:, k] = False
        if len(rows) == 0:
            continue
        if len(xs) == 0:
            nb = k + 1 if zc < 0 else k - 1
            xs = np.nonzero(solid[:, y_from:, nb].any(axis=1))[0]
        y0, y1 = rows.min() // 2, rows.max() // 2
        for y in range(y0, y1 + 1):
            xr = np.nonzero(cov[:, y, k] > 0.15)[0]
            use = xr if len(xr) else xs
            solid[use.min():use.max() + 1, y, k] = True
        for i in range(W):
            if solid[i, y1, k] and rows.max() % 2 == 0:
                slabs[(i, y1, k)] = "bottom"
            if solid[i, y0, k] and rows.min() % 2 == 1 and y0 != y1 and not body:
                slabs[(i, y0, k)] = "top"
    out = {}
    for i, y, k in zip(*np.nonzero(solid)):
        key = (int(i) - 4, int(y), int(k) - 16)
        if (i, y, k) in slabs:
            out[key] = f"smooth_quartz_slab[type={slabs[(i, y, k)]},waterlogged=false]"
        else:
            out[key] = "smooth_quartz"
    # the heart on the chest, a little to the statue's right of centre
    chest = [x for (x, y, z) in out if y == 22 and z == 1]
    if chest:
        out[(max(chest), 22, 1)] = "chiseled_quartz_block"
    # the head is three blocks wide: half blocks carry its crown, brow and beard
    oct_ = fine.reshape(W, 2, 2, H, 2, 2, L, 2, 2).mean(axis=(2, 5, 8)).transpose(0, 2, 4, 1, 3, 5)
    shapes = shape_table()
    for i in range(W):
        for y in range(25, H):
            for k in range(L):
                if abs(k + lo[2] + 0.5) >= 2.5 or cov[i, y, k] < 0.15:
                    continue
                o = oct_[i, y, k].reshape(8)
                plain = np.abs(o - (1.0 if solid[i, y, k] else 0.0)).sum()
                best = None
                for name, m, pen in shapes:
                    c = np.abs(o - m).sum() + pen
                    if c + 0.8 <= plain and (best is None or c < best[0]):
                        best = (c, name)
                if best:
                    out[(i - 4, y, k - 16)] = best[1]
    return out


def shape_table():
    """Slabs and every stairs state with the octants they fill, index ox*4 + oy*2 + oz."""
    def mask(cells):
        m = np.zeros(8)
        for ox, oy, oz in cells:
            m[ox * 4 + oy * 2 + oz] = 1
        return m
    out = [("smooth_quartz_slab[type=bottom,waterlogged=false]", mask([(a, 0, b) for a in (0, 1) for b in (0, 1)]), 0.3),
           ("smooth_quartz_slab[type=top,waterlogged=false]", mask([(a, 1, b) for a in (0, 1) for b in (0, 1)]), 0.3)]
    front = {"east": [(1, 0), (1, 1)], "west": [(0, 0), (0, 1)], "south": [(0, 1), (1, 1)], "north": [(0, 0), (1, 0)]}
    left = {"east": "north", "north": "west", "west": "south", "south": "east"}
    right = {v: k for k, v in left.items()}
    back = {"east": "west", "west": "east", "north": "south", "south": "north"}
    for f in front:
        quads = {
            "straight": front[f],
            "outer_left": [q for q in front[f] if q in front[left[f]]],
            "outer_right": [q for q in front[f] if q in front[right[f]]],
            "inner_left": front[f] + [q for q in front[back[f]] if q in front[left[f]]],
            "inner_right": front[f] + [q for q in front[back[f]] if q in front[right[f]]],
        }
        for shape, qs in quads.items():
            for half, full_y, part_y in (("bottom", 0, 1), ("top", 1, 0)):
                cells = [(a, full_y, b) for a in (0, 1) for b in (0, 1)] + [(a, part_y, b) for a, b in qs]
                out.append((f"smooth_quartz_stairs[facing={f},half={half},shape={shape},waterlogged=false]", mask(cells), 0.6))
    return out


# ---------------------------------------------------------------- pedestal


def octagon(r, c):
    """Cells of a (2r+1)-wide square with corners cut c deep."""
    return [(x, z) for x in range(-r, r + 1) for z in range(-r, r + 1) if abs(x) + abs(z) <= 2 * r - c]


def pedestal_blocks():
    """Pedestal and chapel in the design frame, y from the platform floor (layer 1 is the first
    above it). Dark granite cladding, a stepped plinth, a pale cap; the chapel door is at the back."""
    out = {}
    shaft = set(octagon(4, 1))
    for x, z in octagon(5, 2):
        if (x, z) in shaft:
            continue
        # the plinth leans in from a course of stairs, the cap juts out over one
        if abs(x) == 5 and abs(z) < 4:
            facing = "east" if x < 0 else "west"
        elif abs(z) == 5 and abs(x) < 4:
            facing = "south" if z < 0 else "north"
        else:
            facing = None
        if facing:
            out[(x, 1, z)] = f"polished_deepslate_stairs[facing={facing},half=bottom,shape=straight,waterlogged=false]"
            out[(x, PEDESTAL_H, z)] = f"smooth_quartz_stairs[facing={facing},half=top,shape=straight,waterlogged=false]"
        else:
            out[(x, 1, z)] = "polished_deepslate_slab[type=bottom,waterlogged=false]"
            out[(x, PEDESTAL_H, z)] = "smooth_quartz_slab[type=top,waterlogged=false]"
    room = set(octagon(2, 1))
    for y in range(1, PEDESTAL_H + 1):
        for x, z in shaft:
            if y == PEDESTAL_H:
                out[(x, y, z)] = "smooth_quartz"
            elif (x, z) in room and 2 <= y <= 5:
                continue
            elif max(abs(x), abs(z)) <= 3 and 2 <= y <= 6:
                out[(x, y, z)] = "white_concrete" if y < 6 else "smooth_quartz"
            elif y == 1 and max(abs(x), abs(z)) <= 3:
                out[(x, y, z)] = "polished_diorite"
            else:
                out[(x, y, z)] = "polished_deepslate"
    for y in (2, 3):
        out[(-3, y, 0)] = None
        half = "lower" if y == 2 else "upper"
        out[(-4, y, 0)] = f"iron_door[facing=east,half={half},hinge=left,open=false,powered=false]"
    out[(-4, 4, 0)] = "chiseled_quartz_block"
    out[(2, 2, 0)] = "chiseled_quartz_block"
    out[(2, 3, 0)] = "lantern[hanging=false,waterlogged=false]"
    for z in (-1, 1):
        out[(2, 2, z)] = "smooth_quartz_slab[type=bottom,waterlogged=false]"
    for x in (-1, 0):
        for z in (-1, 1):
            out[(x, 2, z)] = "dark_oak_stairs[facing=west,half=bottom,shape=straight,waterlogged=false]"
    out[(0, 5, 0)] = "lantern[hanging=true,waterlogged=false]"
    # the chapel is lit evenly, not just at the altar
    out[(-1, 4, 0)] = "light[level=13,waterlogged=false]"
    return out


def rotate(blocks, angle):
    """Turn design-frame columns about (0, 0) by `angle` degrees clockwise from above with three
    shears, so every block lands on its own cell and parts keep their thickness."""
    phi = math.radians(angle)
    a, b = -math.tan(phi / 2), math.sin(phi)
    out = {}
    for (x, y, z), name in blocks.items():
        x1 = x + round(a * z)
        z1 = z + round(b * x1)
        x2 = x1 + round(a * z1)
        out[(x2, y, z1)] = name
    return out


# ---------------------------------------------------------------- terrace

# Outline of the mapped pedestrian area (way 141708228), widened to the paved escalator landing
# north-east of it in the orthophotos. Metres from the monument node, x east, z south.
PLATFORM = [(-7.2, -9.0), (-12.1, -1.8), (-12.7, 0.1), (-12.8, 2.0), (-12.2, 4.0), (-10.9, 5.7),
            (-4.2, 10.6), (0.5, 10.5), (0.6, 13.0), (1.9, 12.9), (3.7, 11.9), (6.1, 9.6), (9.4, 4.9),
            (11.0, 2.6), (12.7, 0.8), (14.0, 0.1), (15.4, -0.3), (23.6, -1.6), (24.1, -0.6),
            (24.9, 0.1), (26.4, 0.4), (33.6, -0.6), (34.4, -1.1), (35.1, -1.9), (35.6, -3.5),
            (47.8, -5.4), (48.7, -5.9), (49.4, -6.9), (49.5, -8.1), (49.0, -9.2), (48.0, -9.9),
            (46.7, -10.0), (34.1, -8.0), (30.8, -9.8), (25.2, -9.1), (24.4, -8.7), (23.8, -8.1),
            (23.3, -7.2), (23.2, -6.1), (14.1, -4.6), (13.8, -6.5), (13.4, -10.2), (8.6, -16.6),
            (4.6, -16.6), (4.6, -12.2), (-1.5, -12.2), (-4.1, -12.0), (-4.0, -9.5)]
# Ring walk round the west bastion and the landing at the top of the station escalators, a
# storey and a half down (LiDAR: 697 m against the platform's 701 m, 5 m cells).
RING = [(-12.1, -1.8), (-12.7, 0.1), (-12.8, 2.0), (-12.2, 4.0), (-10.9, 5.7), (-4.2, 10.6),
        (-3.6, 12.9), (-5.6, 13.4), (-13.0, 8.4), (-15.6, 8.6), (-17.3, 4.4), (-17.9, 0.0),
        (-17.5, -3.6), (-19.2, -6.4), (-19.4, -17.4), (-10.8, -17.4), (-10.8, -12.4), (-7.2, -9.0)]
RING_LEVEL = -5
ESCALATOR_Z = (-16.4, -12.6)


def inside(poly, x, z):
    n, c = len(poly), False
    for i in range(n):
        x1, z1 = poly[i]
        x2, z2 = poly[(i + 1) % n]
        if (z1 > z) != (z2 > z) and x < x1 + (z - z1) * (x2 - x1) / (z2 - z1):
            c = not c
    return c


def seg_param(p, a, b):
    ax, az = a
    bx, bz = b
    dx, dz = bx - ax, bz - az
    t = ((p[0] - ax) * dx + (p[1] - az) * dz) / (dx * dx + dz * dz)
    t = min(1.0, max(0.0, t))
    cx, cz = ax + t * dx, az + t * dz
    return t, math.hypot(p[0] - cx, p[1] - cz)


def facing_of(dx, dz):
    if abs(dx) >= abs(dz):
        return "east" if dx > 0 else "west"
    return "south" if dz > 0 else "north"


def terrace():
    """Per column: (level, kind, facing). Level is the walking floor's block layer from L0."""
    cells = {}
    for i in range(WIDTH):
        for k in range(LENGTH):
            x, z = i - OX, k - OZ
            if inside(PLATFORM, x, z):
                if x > WALK_END or (ESCALATOR_Z[0] <= z <= ESCALATOR_Z[1] and x < 4.6):
                    continue
                cells[(i, k)] = (0, "floor", None)
            elif inside(RING, x, z):
                cells[(i, k)] = (RING_LEVEL, "floor", None)
    flights = [
        # up from the ring to the platform's north-west corner (way 1021319653)
        ((-12.4, -3.1), (-8.0, -10.2), RING_LEVEL, 0, 1.4, 1),
        # up from the south-west walk to the platform's south tip (way 1021319652)
        ((-4.6, 12.1), (0.6, 11.9), RING_LEVEL, 0, 1.1, 1),
        # down from the ring towards the restaurant terrace (way 1021319650)
        ((-14.3, 7.4), (-16.6, 13.4), RING_LEVEL, RING_LEVEL - 5, 1.3, 1),
        # escalator from the landing up to the platform's north-east corner (way 1021319646)
        ((-10.6, -14.5), (4.4, -14.5), RING_LEVEL, 0, 1.9, 2),
    ]
    for a, b, lv0, lv1, half, per in flights:
        length = math.hypot(b[0] - a[0], b[1] - a[1])
        low, high = min(lv0, lv1), max(lv0, lv1)
        steps = high - low
        upward = (b[0] - a[0], b[1] - a[1]) if lv1 > lv0 else (a[0] - b[0], a[1] - b[1])
        face = facing_of(*upward)
        run = steps * per
        start = (length - run) / 2
        kind = "stairs" if per == 1 else "escalator"
        for i in range(WIDTH):
            for k in range(LENGTH):
                x, z = i - OX, k - OZ
                t, d = seg_param((x, z), a, b)
                if d > half or t <= 0.0 or t >= 1.0:
                    continue
                along = t * length if lv1 > lv0 else (1 - t) * length
                if along < start:
                    cells[(i, k)] = (low, "floor", None)
                elif along >= start + run:
                    cells[(i, k)] = (high, "floor", None)
                else:
                    n = int(along - start)
                    # escalators climb a half block per cell: a slab, then the next full layer
                    if per == 2:
                        cells[(i, k)] = (low + n // 2 + 1, kind if n % 2 == 0 else "floor", face)
                    else:
                        cells[(i, k)] = (low + n + 1, kind, face)
    return cells


def rock(i, k, y):
    """Granite of the summit dome: stone with broad streaks of andesite and tuff, never speckle."""
    v = math.sin(i * 0.31 + y * 0.17) + math.sin(k * 0.27 - y * 0.23) + 0.6 * math.sin((i + k) * 0.13 + y * 0.41)
    return "tuff" if v > 1.3 else "andesite" if v < -1.0 else "stone"


def mix(i, k, y, *pairs):
    """A calm per-cell mosaic: the first block whose share the cell's hash falls under."""
    h = ((i * 73856093) ^ (k * 19349663) ^ (y * 83492791)) % 100
    for name, share in pairs:
        if h < share:
            return name
        h -= share
    return pairs[-1][0]


def floor_block(x, z):
    """Pale paving with the dark bands of the orthophotos: down the walkway and round the pedestal."""
    if x > 13.0 and seg_param((x, z), (13.5, -2.6), (23.4, -3.9))[1] < 1.2:
        return "polished_andesite"
    px, pz = x - PED[0], z - PED[1]
    if abs(px) <= 7 and abs(pz) <= 7 and abs(px) + abs(pz) <= 11 and (
            max(abs(px), abs(pz)) == 7 or abs(px) + abs(pz) == 11):
        return "polished_andesite"
    # worn granite paving, two close greys
    return mix(int(x), int(z), 0, ("smooth_stone", 80), ("stone", 20))


def wall_state(conn):
    """A balustrade run; the post rule is the game's own, so block updates leave it unchanged."""
    n, e, s, w = conn
    v = lambda c: "low" if c else "none"
    post = not ((n and s and not e and not w) or (e and w and not n and not s))
    return (f"diorite_wall[east={v(e)},north={v(n)},south={v(s)},"
            f"up={'true' if post else 'false'},waterlogged=false,west={v(w)}]")


def build(statue, pedestal):
    g = Grid(WIDTH, HEIGHT, LENGTH)
    cells = terrace()
    top = {}
    for (i, k), (lv, kind, face) in cells.items():
        top[(i, k)] = L0 + lv
    nb4 = ((0, -1), (1, 0), (0, 1), (-1, 0))

    def lower_side(i, k):
        y = top[(i, k)]
        return [top.get((i + dx, k + dz), -99) < y - (1 if cells.get((i + dx, k + dz), (0, ""))[1] == "stairs" else 0)
                for dx, dz in nb4]

    # summit rock outside the terrace on the cliff sides, west and north kept clear for the station
    edge = [(i, k) for (i, k) in cells if cells[(i, k)][0] >= -2 and any(lower_side(i, k))]
    ex = np.array([e[0] for e in edge], float)
    ek = np.array([e[1] for e in edge], float)
    ey = np.array([top[e] for e in edge], float)
    skirt = {}
    for i in range(WIDTH):
        x = i - OX
        if x < -13:
            continue
        for k in range(LENGTH):
            z = k - OZ
            if (i, k) in cells or z < -22:
                continue
            d = np.hypot(ex - i, ek - k)
            j = int(np.argmin(d))
            bumps = 1.6 * math.sin(i * 0.37 + k * 0.11) * math.sin(k * 0.29 - i * 0.07)
            h = int(math.floor(ey[j] - 2 - SKIRT_SLOPE * (d[j] - 0.5) + bumps))
            if h >= L0 - SKIRT:
                skirt[(i, k)] = h
    for (i, k), h in skirt.items():
        for y in range(0, h + 1):
            g.set(i, y, k, rock(i, k, y) if y > 0 else "stone")
    # substructure, masonry where a terrace drops to the next, a coping course over the rock
    for (i, k), (lv, kind, face) in cells.items():
        y0 = top[(i, k)]
        wall_to = y0
        for dx, dz in nb4:
            q = (i + dx, k + dz)
            if q in top:
                if top[q] < y0 - 1:
                    wall_to = min(wall_to, top[q])
            else:
                wall_to = min(wall_to, y0 - 2)
        for y in range(0, y0):
            if y >= wall_to:
                g.set(i, y, k, mix(i, k, y, ("stone_bricks", 82), ("cracked_stone_bricks", 11), ("mossy_stone_bricks", 7)))
            else:
                g.set(i, y, k, rock(i, k, y) if 0 < y < y0 - 3 else "stone")
        x, z = i - OX, k - OZ
        if kind == "stairs":
            g.set(i, y0, k, f"polished_andesite_stairs[facing={face},half=bottom,shape=straight,waterlogged=false]")
            g.set(i, y0 - 1, k, "stone")
        elif kind == "escalator":
            g.set(i, y0, k, "polished_andesite_slab[type=bottom,waterlogged=false]")
        else:
            g.set(i, y0, k, floor_block(x, z))
    # balustrades on floor edges that fall away, piers at turns and every fourth block
    rail = {}
    for (i, k), (lv, kind, face) in cells.items():
        if kind != "floor":
            continue
        if any(top.get((i + dx, k + dz), -99) < top[(i, k)] and
               not (cells.get((i + dx, k + dz), (0, ""))[1] in ("stairs", "escalator") and
                    top[(i + dx, k + dz)] >= top[(i, k)] - 1)
               for dx, dz in nb4):
            rail[(i, k)] = top[(i, k)] + 1
    # a diagonal outline only touches at corners: fill the inner step so the rail joins face to face
    for (i, k), y in list(rail.items()):
        for dx, dz in ((1, 1), (1, -1), (-1, 1), (-1, -1)):
            if rail.get((i + dx, k + dz)) != y or y in (rail.get((i + dx, k)), rail.get((i, k + dz))):
                continue
            for c in ((i + dx, k), (i, k + dz)):
                if cells.get(c, (0, ""))[1] == "floor" and top[c] == y - 1:
                    rail[c] = y
                    break
    def shape(i, k):
        y = rail.get((i, k))
        n, e, s, w = (rail.get((i + dx, k + dz)) == y for dx, dz in nb4)
        if (n and s and not e and not w) or (e and w and not n and not s):
            return "straight"
        return "bend" if n + e + s + w == 2 else "end"

    for (i, k), y in rail.items():
        conn = tuple(rail.get((i + dx, k + dz)) == y for dx, dz in nb4)
        n, e, s, w = conn
        form = shape(i, k)
        straight = form == "straight"
        along = k if (n and s) else i
        # piers at true corners, ends and every fourth block; stepped diagonals stay one wall
        corner = form == "bend" and all(
            shape(i + dx, k + dz) == "straight" for (dx, dz), c in zip(nb4, conn) if c)
        if form == "end" or corner or (straight and along % 4 == 0) or (
                form == "bend" and (i + k) % 4 == 0 and (i - k) % 4 == 0):
            g.set(i, y, k, "polished_diorite")
            # the terrace's lamps stand on every other pier
            if straight and along % 8 == 0:
                g.set(i, y + 1, k, "lantern[hanging=false,waterlogged=false]")
        else:
            g.set(i, y, k, wall_state(conn))
    # glass balustrades either side of the escalators
    for (i, k), (lv, kind, face) in cells.items():
        x, z = i - OX, k - OZ
        if -10.6 < x < 4.4 and z in (-16, -13):
            y = top[(i, k)] + 1
            g.set(i, y, k, "glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]")
    # pedestal and statue, turned onto the statue's facing
    blocks = {}
    for (x, y, z), name in pedestal.items():
        blocks[(x, L0 + y, z)] = name
    for (x, y, z), name in statue.items():
        blocks[(x, L0 + PEDESTAL_H + 1 + y, z)] = name
    rot = rotate(blocks, FACING - 90.0)
    # the shears move rows apart: keep the cladding outermost and the doorway open to the chapel
    shaft = {(x, z) for (x, y, z), n in rot.items() if y == L0 + PEDESTAL_H - 1 and n}
    for (x, y, z), n in list(rot.items()):
        if L0 + 2 <= y <= L0 + PEDESTAL_H - 2 and n in ("white_concrete", "smooth_quartz") and any(
                (x + a, z + b) not in shaft for a, b in ((1, 0), (-1, 0), (0, 1), (0, -1))):
            rot[(x, y, z)] = "polished_deepslate"
    for (x, y, z), n in list(rot.items()):
        if n and n.startswith("iron_door") and "half=lower" in n:
            for dx in range(1, 4):
                if not rot.get((x + dx, y, z)):
                    break
                rot[(x + dx, y, z)] = rot[(x + dx, y + 1, z)] = None
    for (x, y, z), name in rot.items():
        i, k = OX + PED[0] + x, OZ + PED[1] + z
        if name is None:
            g.ids[i, y, k] = 0
        else:
            g.set(i, y, k, name)
    floodlight(g)
    return g


def floodlight(g):
    """The statue and pedestal are floodlit at night from the terrace: sparse invisible light in
    the air beside their faces, no two within four blocks, so every face reads lit."""
    light = g.pid("light[level=13,waterlogged=false]")
    lit = np.zeros(g.ids.shape, bool)
    lit[:, L0 + 1:, :] = True
    body = np.zeros(g.ids.shape, bool)
    for name, idx in g.index.items():
        if any(m in name for m in ("smooth_quartz", "chiseled_quartz", "polished_deepslate")):
            body |= g.ids == idx
    body &= lit
    near = np.zeros_like(body)
    for ax in (0, 2):
        for d in (1, -1):
            near |= np.roll(body, d, axis=ax)
    near |= np.roll(body, 1, axis=1)
    cand = near & (g.ids == 0) & lit
    placed = []
    for i, y, k in sorted(zip(*np.nonzero(cand)), key=lambda c: (c[1], c[0], c[2])):
        if all(max(abs(i - a), abs(y - b), abs(k - c)) > 4 for a, b, c in placed):
            placed.append((i, y, k))
            g.ids[i, y, k] = light


def main():
    cache = sys.argv[1] if len(sys.argv) > 1 else os.environ.get(
        "ARNIS_CACHE", os.path.expanduser("~/.cache/arnis-landmarks"))
    stl = fetch(STL_URL, cache, "christ_the_redeemer.stl")
    photo = fetch(PHOTO_URL, cache, "cristo_front.jpg")
    g = build(statue_blocks(stl, photo), pedestal_blocks())
    n = g.save(OUT)
    print(f"{OUT}: {WIDTH}x{HEIGHT}x{LENGTH}, {n} block states, {int((g.ids > 0).sum())} blocks")


if __name__ == "__main__":
    main()
