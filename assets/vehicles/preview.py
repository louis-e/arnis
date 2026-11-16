#!/usr/bin/env python3
"""Draws a model from six sides into one PNG, for a quick look while designing.

    python preview.py MODEL_ID OUT.png [--zoom N] [--views front34,side,...]

Orthographic, nearest-neighbour, lit like Blockbench's viewport. It shares kit.py's face
conventions, so open the model in Blockbench to be sure of those. Needs numpy.
"""
import math
import os
import sys

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import kit  # noqa: E402
from build import models  # noqa: E402

VIEWS = {
    "front34": (215, 24),
    "rear34": (35, 22),
    "side": (90, 0),
    "left": (270, 0),
    "front": (180, 4),
    "rear": (0, 4),
    "top": (180, 89.9),
    "under": (180, -89.9),
}


def quads(built):
    tex = np.array(built.texture.convert("RGB"))
    out = []
    for k, f in enumerate(built.faces):
        if f.variant == "bright":
            continue
        box = f.box
        spec = kit.FACES[f.name]
        ua, us = spec["u"]
        va, vs = spec["v"]
        pa, ps = spec["plane"]
        o = [0.0, 0.0, 0.0]
        o[pa] = box.to[pa] if ps else box.frm[pa]
        o[ua] = box.frm[ua] if us > 0 else box.to[ua]
        o[va] = box.frm[va] if vs > 0 else box.to[va]
        U = [0.0, 0.0, 0.0]
        U[ua] = us * (box.to[ua] - box.frm[ua])
        V = [0.0, 0.0, 0.0]
        V[va] = vs * (box.to[va] - box.frm[va])
        O = np.array(box.to_world(o))
        Uw = np.array(box.dir_to_world(U))
        Vw = np.array(box.dir_to_world(V))
        n = np.array(box.dir_to_world(spec["normal"]))
        u, v = built.spots[k]
        out.append((O, Uw, Vw, n, (u, v, u + f.su, v + f.sv)))
    return out, tex


def render(built, yaw, pitch, scale, size, centre):
    qs, tex = quads(built)
    W, H = size
    cy, sy = math.cos(math.radians(yaw)), math.sin(math.radians(yaw))
    cp, sp = math.cos(math.radians(pitch)), math.sin(math.radians(pitch))
    forward = np.array([sy * cp, sp, cy * cp])
    right = np.array([cy, 0, -sy])
    up = np.cross(forward, right)
    light = np.array([0.35, 0.85, -0.4])
    light /= np.linalg.norm(light)
    colour = np.zeros((H, W, 3), float)
    colour[:] = (205, 214, 222)
    depth = np.full((H, W), -1e9)
    ys, xs = np.mgrid[0:H, 0:W]
    px = (xs + 0.5 - W / 2) / scale
    py = (H / 2 - ys - 0.5) / scale
    for O, U, V, n, uv in qs:
        if n @ forward <= 1e-6:
            continue
        o2 = np.array([(O - centre) @ right, (O - centre) @ up])
        u2 = np.array([U @ right, U @ up])
        v2 = np.array([V @ right, V @ up])
        M = np.array([[u2[0], v2[0]], [u2[1], v2[1]]])
        if abs(np.linalg.det(M)) < 1e-9:
            continue
        Mi = np.linalg.inv(M)
        cs = [o2, o2 + u2, o2 + v2, o2 + u2 + v2]
        x0 = max(0, int(min(c[0] for c in cs) * scale + W / 2) - 1)
        x1 = min(W, int(max(c[0] for c in cs) * scale + W / 2) + 2)
        y0 = max(0, int(H / 2 - max(c[1] for c in cs) * scale) - 1)
        y1 = min(H, int(H / 2 - min(c[1] for c in cs) * scale) + 2)
        if x0 >= x1 or y0 >= y1:
            continue
        bx = px[y0:y1, x0:x1] - o2[0]
        by = py[y0:y1, x0:x1] - o2[1]
        s = Mi[0, 0] * bx + Mi[0, 1] * by
        t = Mi[1, 0] * bx + Mi[1, 1] * by
        inside = (s >= 0) & (s < 1) & (t >= 0) & (t < 1)
        if not inside.any():
            continue
        d = (O - centre) @ forward + s * (U @ forward) + t * (V @ forward)
        win = inside & (d > depth[y0:y1, x0:x1])
        tu = np.clip((uv[0] + s * (uv[2] - uv[0])).astype(int), 0, tex.shape[1] - 1)
        tv = np.clip((uv[1] + t * (uv[3] - uv[1])).astype(int), 0, tex.shape[0] - 1)
        lit = 0.62 + 0.38 * max(0.0, float(n @ light))
        if n[1] < -0.5:
            lit = 0.5
        block = colour[y0:y1, x0:x1]
        block[win] = tex[tv, tu][win] * lit
        depth[y0:y1, x0:x1][win] = d[win]
    return Image.fromarray(np.clip(colour, 0, 255).astype(np.uint8))


def sheet(built, views, tile=(900, 520), zoom=1.0):
    lo, hi = built.bounds
    centre = np.array([(lo[i] + hi[i]) / 2 for i in range(3)])
    reach = math.dist(lo, hi) / 2
    scale = min(tile) / (2 * reach) * 1.0 * zoom
    tiles = [render(built, *VIEWS[v], scale, tile, centre) for v in views]
    cols = 2 if len(tiles) > 1 else 1
    rows = math.ceil(len(tiles) / cols)
    out = Image.new("RGB", (tile[0] * cols, tile[1] * rows), (255, 255, 255))
    for k, t in enumerate(tiles):
        out.paste(t, ((k % cols) * tile[0], (k // cols) * tile[1]))
    return out


def main(argv):
    mid, path = argv[0], argv[1]
    zoom = float(argv[argv.index("--zoom") + 1]) if "--zoom" in argv else 1.0
    views = argv[argv.index("--views") + 1].split(",") if "--views" in argv else ["front34", "rear34", "side", "top", "front", "rear"]
    model = next(m for m in models() if m.id == mid)
    sheet(kit.Built(model), views, zoom=zoom).save(path)


if __name__ == "__main__":
    main(sys.argv[1:])
