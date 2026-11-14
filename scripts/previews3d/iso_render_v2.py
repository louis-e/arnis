"""3D setting previews: isometric Minecraft-block renderer (doc 19, Pillow + numpy + nbtlib).

Reads Arnis region files, crops a block window and draws a 2:1 isometric diorama:
per-face light, ambient occlusion, deterministic tone noise, depth-graded water,
translucent glass. No edge lines or baked drop shadow (the GUI popover has a CSS
drop-shadow). Colours come from the Arnis map palette parsed out of src/map_renderer.rs.

Crop keys (see previews3d.json): slab=[z centre frac, z width, x frac lo, x frac hi],
box=[x0, x1, z0, z1] absolute, vert=vertical exaggeration, floor=N clamps the diorama
bottom to median ground - N, depth=N draws the cut N blocks below the lowest ground (default 3, caves need ~40), cut="diag" removes every column below the line from the image's top-left to its bottom-right corner (a river cross-section on a diagonal cut face); cut=F (true = 0.5) keeps the first fraction F of the window along z and drops the +z side (bottom-left on screen, framing stays that of the whole window) so
the exposed face along the diagonal is a cross-section (top-left to bottom-right on screen).
Driver: previews3d.py.
"""
import io, os, re, time, gzip, zlib
from pathlib import Path

import nbtlib
import numpy as np
from PIL import Image, ImageDraw

REPO = Path(os.environ.get("ARNIS_REPO", Path(__file__).resolve().parents[2]))
W, H = 720, 540
VERT = 1.2
WATER = (59, 86, 165)


def palette():
    src = (REPO / "src/map_renderer.rs").read_text(encoding="utf-8")
    table = {n: (int(r), int(g), int(b)) for n, r, g, b in
             re.findall(r'\("([a-z0-9_]+)",\s*Rgb\(\[(\d+),\s*(\d+),\s*(\d+)\]\)\)', src)}
    fb = src[src.index("fn get_fallback_color"):src.index("fn get_block_colors")]
    rules = [(n, (int(r), int(g), int(b))) for n, r, g, b in
             re.findall(r'name\.contains\("([a-z_]+)"\)[^{]*\{\s*return Rgb\(\[(\d+),\s*(\d+),\s*(\d+)\]\)', fb)]
    tr = src[src.index("fn is_transparent_block"):src.index("fn get_fallback_color")]
    clear = set(re.findall(r'"([a-z_]+)"', tr)) - {"minecraft:"}
    return table, rules, clear


TABLE, RULES, CLEAR = palette()
_colour = {}


def colour(name):
    if name not in _colour:
        short = name.removeprefix("minecraft:")
        c = TABLE.get(short)
        if c is None:
            c = next((rgb for key, rgb in RULES if key in short), (160, 160, 160))
        _colour[name] = c
    return _colour[name]

def chunks(world):
    for mca in (world / "region").glob("r.*.mca"):
        _, rx, rz, _ = mca.name.split(".")
        head = mca.read_bytes()[:4096]
        for i in range(1024):
            off = int.from_bytes(head[i * 4:i * 4 + 3], "big")
            if off:
                yield int(rx) * 32 + i % 32, int(rz) * 32 + i // 32, mca, off


class Blocks:
    def __init__(self):
        self.names, self.ids, self.solid, self.water, self.glass = [], {}, [], [], []

    def id(self, name):
        if name not in self.ids:
            self.ids[name] = len(self.names)
            self.names.append(name)
            short = name.removeprefix("minecraft:")
            self.water.append(short == "water")
            self.solid.append(short != "water" and short not in CLEAR)
            self.glass.append("glass" in short)
        return self.ids[name]


def read_chunk(mca, off, blocks):
    with open(mca, "rb") as f:
        f.seek(off * 4096)
        n = int.from_bytes(f.read(4), "big")
        comp = f.read(1)[0]
        raw = f.read(n - 1)
    raw = zlib.decompress(raw) if comp == 2 else gzip.decompress(raw) if comp == 1 else raw
    c = nbtlib.File.parse(io.BytesIO(raw))
    secs = {}
    for s in c.get("sections", []):
        bs = s.get("block_states")
        if not bs:
            continue
        pal = np.array([blocks.id(str(e["Name"])) for e in bs["palette"]], dtype=np.uint16)
        if "data" not in bs:
            ids = np.full(4096, pal[0], dtype=np.uint16)
        else:
            bits = max(4, int(np.ceil(np.log2(len(pal)))))
            per = 64 // bits
            longs = np.asarray(bs["data"]).astype(np.int64).view(np.uint64)
            shifts = np.arange(per, dtype=np.uint64) * np.uint64(bits)
            idx = ((longs[:, None] >> shifts) & np.uint64((1 << bits) - 1)).reshape(-1)[:4096]
            ids = pal[idx.astype(np.intp)]
        secs[int(s["Y"])] = ids.reshape(16, 16, 16)
    if not secs:
        return None
    lo, hi = min(secs), max(secs)
    air = blocks.id("minecraft:air")
    vol = np.full(((hi - lo + 1) * 16, 16, 16), air, dtype=np.uint16)
    for y, a in secs.items():
        vol[(y - lo) * 16:(y - lo + 1) * 16] = a
    return lo * 16, vol


def column_runs(vol, solid, z, x):
    s = solid[vol[:, z, x]].astype(np.int8)
    d = np.diff(np.concatenate([[0], s, [0]]))
    return list(zip(np.nonzero(d == 1)[0], np.nonzero(d == -1)[0] - 1))


def load(world, slab=None, box=None, margin=0.96, cut=False):
    """{(x, z): (runs [(b, t, top id, under id)], water top or None, water bottom, [(y, glass id)])}."""
    blocks = Blocks()
    all_chunks = list(chunks(world))
    xs = [c[0] for c in all_chunks]
    zs = [c[1] for c in all_chunks]
    x0, x1, z0, z1 = min(xs) * 16, (max(xs) + 1) * 16, min(zs) * 16, (max(zs) + 1) * 16
    if slab:
        zc = int(z0 + (z1 - z0) * slab[0])
        x0, x1 = int(x0 + (x1 - x0) * slab[2]), int(x0 + (x1 - x0) * slab[3])
        z0, z1 = zc - slab[1] // 2, zc + slab[1] // 2
    if box:
        x0, x1, z0, z1 = box
    if cut:  # cut = fraction of the window's z depth kept; render() drops the rest after framing the whole window
        zcut = z0 + int((z1 - z0) * (0.5 if cut is True else 0.5 if cut == "diag" else cut))
    span = (x1 - x0) + (z1 - z0)
    s_est = min(W * margin / span, H * margin * 2 / span)
    k = max(1, round(0.7 / s_est))
    cols, prof = {}, {}  # prof: the cut face's columns block by block (stone, ores, caves show as they are)
    zl = ((zcut - 1) // k) * k if cut and cut != "diag" else None
    for cx, cz, mca, off in all_chunks:
        if cz * 16 + 15 < z0 or cz * 16 >= z1 or cx * 16 + 15 < x0 or cx * 16 >= x1:
            continue
        got = read_chunk(mca, off, blocks)
        if got is None:
            continue
        y0, vol = got
        solid, water, glass = np.array(blocks.solid), np.array(blocks.water), np.array(blocks.glass)
        for z in range(16):
            wz = cz * 16 + z
            if wz % k or not z0 <= wz < z1:
                continue
            for x in range(16):
                wx = cx * 16 + x
                if wx % k or not x0 <= wx < x1:
                    continue
                runs = []
                for b, t in column_runs(vol, solid, z, x):
                    top = int(vol[t, z, x])
                    if blocks.names[top].endswith("bedrock"):  # c35 floating bedrock floor
                        continue
                    under = int(vol[t - 1, z, x]) if t > b else top
                    runs.append((y0 + b, y0 + t, top, under))
                col = vol[:, z, x]
                if cut and (cut == "diag" or wz == zl):
                    prof[(wx, wz)] = (y0, col.copy())
                wcol = np.nonzero(water[col])[0]
                gl = [(y0 + int(y), int(col[y])) for y in np.nonzero(glass[col])[0]]
                cols[(wx, wz)] = (runs, y0 + int(wcol[-1]) if len(wcol) else None,
                                  y0 + int(wcol[0]) if len(wcol) else None, gl)
    return cols, blocks.names, k, prof, zl


# --- per-cell tones -------------------------------------------------------------

_crc = {}




def name_hash(name):
    if name not in _crc:
        _crc[name] = zlib.crc32(name.encode())
    return _crc[name]


def hash01(*v):
    """Deterministic 0..1 from ints (Python's hash() of str is salted per run, crc32 is not)."""
    h = 2166136261
    for a in v:
        h = ((h ^ (int(a) & 0xFFFFFFFF)) * 16777619) & 0xFFFFFFFF
    h ^= h >> 15
    h = (h * 2246822519) & 0xFFFFFFFF
    h ^= h >> 13
    return h / 0xFFFFFFFF


SMOOTH = ("concrete", "wool", "terracotta", "glass", "quartz", "smooth", "iron", "white")


def amp(name):
    return 0.035 if any(m in name for m in SMOOTH) else 0.075




def render(world, slab=None, vert=VERT, box=None, ss=2, floor=None, cut=False, depth=3):
    t0 = time.time()
    cols, names, k, prof, zl = load(world, slab=slab, box=box, cut=cut)
    t1 = time.time()
    keys = list(cols)
    ground = {key: (cols[key][0][0][1] if cols[key][0] else -64) for key in keys}
    base = min(ground.values()) - depth
    if floor:  # one deep pit (subway stairs) would stretch the whole cut face down
        base = max(base, int(np.median(list(ground.values()))) - floor)
    X, Ytop, Ybot = [], [], []
    for (x, z) in keys:
        runs, wt, _, gl = cols[(x, z)]
        top = max([r[1] for r in runs] + [g[0] for g in gl] + ([wt] if wt is not None else []) + [base])
        X += [x - z - k, x - z + k]
        Ytop.append((x + z) / 2 - (top + 1) * vert)
        Ybot.append((x + z + 2 * k) / 2 - base * vert)
    xmin, xmax, ymin, ymax = min(X), max(X), min(Ytop), max(Ybot)
    s = min(W * 0.96 / (xmax - xmin), H * 0.96 / (ymax - ymin))
    S = s * ss
    cw, ch = W * ss, H * ss
    ox = cw / 2 - (xmin + xmax) / 2 * S
    oy = ch / 2 - (ymin + ymax) / 2 * S
    if cut:  # framed on the whole window; the +z side (bottom-left on screen) is now removed and leaves the background
        mid = int(np.median(list(ground.values())))  # one height for every column, so the cut is a clean plane
        for q in keys:
            x, z = q
            if cut == "diag":  # below the line from the image's top-left to its bottom-right corner
                sx = ((x - z + k) * S + ox) / ss
                sy = ((x + z + k) * S / 2 - (mid + 1) * S * vert + oy) / ss
                gone = sy / H > sx / W
            else:
                gone = q[1] > zl
            if gone:
                del cols[q], ground[q]
        keys = list(cols)
    # Cells per face edge: about one per 4 output px of a block's width, 1..3.
    n = max(1, min(3, round(2 * s * k / 4)))

    img = Image.new("RGBA", (cw, ch), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img, "RGBA")

    L_TOP, L_LEFT, L_RIGHT = (1.0, 1.0, 1.0), (0.86, 0.84, 0.79), (0.58, 0.61, 0.67)

    def P(x, z, h):
        return ((x - z) * S + ox, (x + z) * S / 2 - h * S * vert + oy)

    def col(c, light, f=1.0, a=255):
        return (min(255, int(c[0] * light[0] * f)), min(255, int(c[1] * light[1] * f)),
                min(255, int(c[2] * light[2] * f)), a)

    def occ(x, z, y):
        c = cols.get((x, z))
        return c is not None and any(b <= y <= t for b, t, _, _ in c[0])

    def tone(name, face, x, y, z, i, j):
        r = hash01(name_hash(name), x, y, z, face, i, j)
        return 1.0 + amp(name) * (-1 if r < 0.3 else 1 if r > 0.75 else 0)  # 3 tones

    def top_face(x, z, hh, name, c, a=255):
        """Top face of the block whose top is at hh, n x n cells with noise + AO."""
        ao = {}
        if a == 255:
            for dx in (-1, 0, 1):
                for dz in (-1, 0, 1):
                    if (dx or dz) and occ(x + dx * k, z + dz * k, hh):
                        ao[(dx, dz)] = True
        for i in range(n):
            for j in range(n):
                f = tone(name, 0, x, hh, z, i, j) if a == 255 else 1.0
                if ao:
                    ex = -1 if i == 0 else 1 if i == n - 1 else 0
                    ez = -1 if j == 0 else 1 if j == n - 1 else 0
                    hits = sum(1 for d in {(ex, 0), (0, ez), (ex, ez)} if d != (0, 0) and ao.get(d))
                    f *= (0.80, 0.72, 0.66)[hits - 1] if hits else 1.0
                xa, xb = x + k * i / n, x + k * (i + 1) / n
                za, zb = z + k * j / n, z + k * (j + 1) / n
                draw.polygon([P(xa, za, hh), P(xb, za, hh), P(xb, zb, hh), P(xa, zb, hh)], fill=col(c, L_TOP, f, a))

    def side_cells(x, z, y, top_y, name, c, side, a=255, fdepth=None):
        """One block row (y..y+1) of the +z (side 0, left) or +x (side 1, right) face."""
        light = L_LEFT if side == 0 else L_RIGHT
        depth = top_y - y
        g = fdepth(depth) if fdepth else max(0.78, 1 - 0.022 * depth)
        nv = n if a == 255 else 1
        for i in range(nv):
            for r in range(nv):
                f = g * (tone(name, 1 + side, x, y, z, i, r) if a == 255 else 1.0)
                ya, yb = y + 1 - r / nv, y + 1 - (r + 1) / nv
                ua, ub = k * i / nv, k * (i + 1) / nv
                if side == 0:
                    q = [P(x + ua, z + k, ya), P(x + ub, z + k, ya), P(x + ub, z + k, yb), P(x + ua, z + k, yb)]
                else:
                    q = [P(x + k, z + ua, ya), P(x + k, z + ub, ya), P(x + k, z + ub, yb), P(x + k, z + ua, yb)]
                draw.polygon(q, fill=col(c, light, f, a))

    nb = 0
    for (x, z) in sorted(keys, key=lambda q: (q[0] + q[1], q[0])):
        runs, wt, wb, gl = cols[(x, z)]
        front = [ground.get((x + k, z)), ground.get((x, z + k))]
        items = [(r[0], 0, r) for r in runs] + [(g[0], 1, g) for g in gl]
        if cut and ((x, z + k) not in cols if cut == "diag" else z == zl):  # air between runs (caves) shows on the cut face as dark openings
            for (_, t0, _, _), (b1, _, _, _) in zip(runs, runs[1:]):
                if b1 > t0 + 1:
                    draw.polygon([P(x, z + k, b1), P(x + k, z + k, b1), P(x + k, z + k, t0 + 1), P(x, z + k, t0 + 1)],
                                 fill=col((26, 24, 30), L_LEFT))
        for _, kind, it in sorted(items, key=lambda q: q[0]):
            if kind == 1:  # glass: translucent box
                y, gid = it
                gc = (205, 232, 240)
                for side in (0, 1):
                    nx, nz = (x, z + k) if side == 0 else (x + k, z)
                    if not occ(nx, nz, y):
                        side_cells(x, z, y, y, names[gid], gc, side, a=80)
                top_face(x, z, y + 1, names[gid], gc, a=95)
                continue
            b, t, top, under = it
            h = t + 1
            first = it is runs[0]
            if first:
                b = base if None in front else max(base, min(front) - 1)
                h = max(h, base + 1)  # a pit below the floor (see floor=) gets its bottom at the floor
            ct, cu = colour(names[top]), colour(names[under])
            nb += 1
            lo = max(b, base)
            for y in range(lo, h):
                nm, c = (names[top], ct) if y == h - 1 else (names[under], cu)
                if (x, z) in prof and 0 <= y - prof[(x, z)][0] < len(prof[(x, z)][1]):
                    nm = names[prof[(x, z)][1][y - prof[(x, z)][0]]]
                    c = colour(nm)
                for side in (0, 1):
                    nx, nz = (x, z + k) if side == 0 else (x + k, z)
                    if occ(nx, nz, y):
                        continue  # hidden behind the neighbour's solid block
                    side_cells(x, z, y, h - 1, nm, c, side)
            top_face(x, z, h, names[top], ct)
            if first and wt is not None and wt >= t:
                depth = wt - t
                wet = (cols.get((x, z + k), (None, None))[1] is None, cols.get((x + k, z), (None, None))[1] is None)
                # Depth-graded: colour slides shallow -> deep, alpha rises; cut faces
                # get darker toward the bed so the profile reads.
                d = min(1.0, depth / 10)
                wc = tuple(int(sh * (1 - d) + dp * d) for sh, dp in zip((92, 150, 205), (30, 58, 130)))
                for side in (0, 1):
                    if wet[side]:
                        for y in range(h, wt + 1):
                            dd = min(1.0, (wt - y) / 10)
                            c2 = tuple(int(sh * (1 - dd) + dp * dd) for sh, dp in zip((92, 150, 205), (25, 45, 110)))
                            side_cells(x, z, y, wt, "minecraft:water", c2, side, a=int(90 + 90 * dd), fdepth=lambda _: 1.0)
                a = int(110 + 120 * d)
                f = 1.0 + 0.05 * (hash01(x, z, 7) - 0.5)
                draw.polygon([P(x, z, wt + 1), P(x + k, z, wt + 1), P(x + k, z + k, wt + 1), P(x, z + k, wt + 1)],
                             fill=col(wc, L_TOP, f, a))

    out = img.resize((W, H), Image.Resampling.LANCZOS) if ss > 1 else img
    t2 = time.time()
    print(f"  {len(cols)} columns (step {k}, cells {n}), {nb} runs, s={s:.2f}; read {t1 - t0:.1f} s, "
          f"draw {t2 - t1:.1f} s", flush=True)
    return out, t2 - t0



def save(img, path, budget_kb=60):
    """WebP q80, stepping the quality down until the file is within budget_kb."""
    path.parent.mkdir(parents=True, exist_ok=True)
    for q in (80, 74, 68, 62, 56, 50):
        img.save(path, "WEBP", quality=q, method=6)
        if path.stat().st_size / 1024 <= budget_kb:
            break
    return path.stat().st_size / 1024
