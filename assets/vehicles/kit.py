"""The vehicle model kit: boxes, a texture painted from the vehicle's own shape, livery
colours, and the files Blockbench, Bedrock and Java read.

A model is a list of boxes in Blockbench's frame: 16 units to a metre, the front towards
-z, the ground (or rail, or waterline) at y=0, +x the vehicle's right side. Each box names
a painter, and every texel of every face asks that painter what lies at its spot, given
the spot in the vehicle's frame and the face it is on. So a panel line or a window band
runs on across neighbouring boxes without anyone lining up UVs. One texel per unit, the
density Minecraft draws blocks at.

A painter answers with a fixed colour, an RGB tuple, or with `Paint(slot, k)`: the
livery's colour for `slot` lightened (k > 0) or darkened (k < 0). The texture is written
twice: `<id>.png` in the preview livery and `<id>.mask.png`, which says per texel which
slot it takes and how much to shade it, so Arnis colours a model for any livery itself.

Boxes may turn about one axis, by at most 45 degrees, which Java takes from 1.21.6 on.
"""
import base64
import hashlib
import io
import json
import math
import os
import uuid

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))

# ---------------------------------------------------------------- colours

with open(os.path.join(HERE, "paints.json")) as f:
    PAINTS = {k: tuple(v) for k, v in json.load(f).items() if not k.startswith("_")}

# Livery slots, as train_bodies.rs' Livery names them. Cars use only "body".
SLOTS = ("body", "lower", "stripe", "door", "roof")


def hexc(h):
    return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))


def shade(c, k):
    """Towards white for k > 0, towards black for k < 0. Arnis repeats this exactly."""
    if k >= 0:
        return tuple(round(v + (255 - v) * k) for v in c)
    return tuple(round(v * (1 + k)) for v in c)


def mix(a, b, t):
    t = max(0.0, min(1.0, t))
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


class Paint:
    """A texel in a livery colour."""
    __slots__ = ("slot", "k")

    def __init__(self, slot="body", k=0.0):
        assert slot in SLOTS, slot
        self.slot = slot
        self.k = max(-1.0, min(1.0, float(k)))

    def rgb(self, livery):
        return shade(PAINTS[livery[self.slot]], self.k)


# Shared colours, so every vehicle's glass, rubber and chrome match.
TRIM = hexc("#1b1b1e")
GLOSS = hexc("#111114")
UNDER = hexc("#18181a")
CHROME = hexc("#d3d7dc")
CHROME_DARK = hexc("#8c9198")
STEEL = hexc("#6e747c")
GLASS_DARK = hexc("#243447")
GLASS_MID = hexc("#36506a")
GLASS_LIGHT = hexc("#5b7590")
GLASS_HI = hexc("#9db6cc")
TIRE = hexc("#1d1d20")
TIRE_SIDE = hexc("#2a2a2e")
TREAD_RIB = hexc("#2c2c30")
TREAD_GROOVE = hexc("#141416")
RIM = hexc("#b3b9c1")
RIM_LIP = hexc("#d5d9de")
RIM_DARK = hexc("#3a3d43")
HUB = hexc("#7a8089")
DRL = hexc("#f7fbff")
LENS = hexc("#c9d4df")
HOUSING = hexc("#3b4048")
HOUSING_DARK = hexc("#2a2e34")
AMBER = hexc("#f39a2b")
TAIL_RED = hexc("#a3141f")
TAIL_BRIGHT = hexc("#ec3b3b")
TAIL_DARK = hexc("#3a0a0f")
REVERSE = hexc("#e6e6e6")
PLATE = hexc("#ecebe4")
PLATE_BLUE = hexc("#2446a8")
PLATE_TEXT = hexc("#55575e")
MIRROR_GLASS = hexc("#9fb2c4")


def glass(y, a, b, top=22.0, bottom=15.5):
    """Tinted glass, lighter towards the top, with short glints of sky. a and b are the
    spot in the face's own texel grid (whole units), so the glints step evenly; top and
    bottom are the heights the gradient runs between."""
    c = mix(GLASS_DARK, GLASS_MID, (y - bottom) / max(1.0, top - bottom))
    ia, ib = math.floor(a), math.floor(b)
    m = (ia + ib) % 13
    if (ia - ib) % 22 < 9:
        if m in (0, 1):
            return GLASS_HI
        if m == 3:
            return GLASS_LIGHT
    return c


def wheel_face(dz, dy, radius=5.0):
    """The outer face of a road wheel: hub, five spokes, rim, tyre. dz and dy from the
    wheel's centre; radius in units."""
    r = math.hypot(dz, dy) * 5.0 / radius
    if r < 1.0:
        return HUB
    if r < 3.2:
        a = math.degrees(math.atan2(dy, dz)) % 72
        return RIM if min(a, 72 - a) < 20 else RIM_DARK
    if r < 3.9:
        return RIM_LIP
    if r < 4.6:
        return TIRE_SIDE
    return TIRE


# ---------------------------------------------------------------- boxes


def _matmul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(3)) for j in range(3)] for i in range(3)]


class Box:
    """A box from `frm` to `to`, turned by `rot` (degrees, one axis) about `origin`.

    `mat` names the model's painter for it, `bone` the Bedrock bone it moves with.
    `lamp` is None, "head" or "glow" (always lit) or "brake" (lit while braking)."""

    def __init__(self, name, frm, to, mat, bone="body", origin=(0, 0, 0), rot=(0, 0, 0), lamp=None, **extra):
        self.name = name
        self.frm = [float(v) for v in frm]
        self.to = [float(v) for v in to]
        assert all(self.to[i] > self.frm[i] for i in range(3)), (name, frm, to)
        self.mat = mat
        self.bone = bone
        self.origin = [float(v) for v in origin]
        self.rot = [float(v) for v in rot]
        assert sum(1 for a in self.rot if a) <= 1, f"{name}: one axis only"
        assert all(abs(a) <= 45.0 + 1e-9 for a in self.rot), f"{name}: Java takes 45 degrees at most"
        assert lamp in (None, "head", "glow", "brake"), lamp
        self.lamp = lamp
        self.extra = extra
        self._m = self._matrix()
        self._aabb = self._bounds()

    def mirrored(self, name=None, bone=None):
        """The same box on the other side of the vehicle (x -> -x)."""
        b = Box(
            name or self.name.replace("right", "left"),
            (-self.to[0], self.frm[1], self.frm[2]),
            (-self.frm[0], self.to[1], self.to[2]),
            self.mat,
            bone or self.bone.replace("right", "left"),
            (-self.origin[0], self.origin[1], self.origin[2]),
            (self.rot[0], -self.rot[1], -self.rot[2]),
            self.lamp,
            **self.extra,
        )
        if "wheel" in b.extra:
            wx, wy, wz = b.extra["wheel"]
            b.extra["wheel"] = (-wx, wy, wz)
        return b

    def moved(self, dx=0.0, dy=0.0, dz=0.0, name=None):
        """The same box shifted, for repeated bays and segments."""
        d = (dx, dy, dz)
        b = Box(
            name or self.name,
            [self.frm[i] + d[i] for i in range(3)],
            [self.to[i] + d[i] for i in range(3)],
            self.mat, self.bone,
            [self.origin[i] + d[i] for i in range(3)],
            self.rot, self.lamp, **self.extra,
        )
        return b

    def _matrix(self):
        # three.js Euler order ZYX, as Blockbench turns a cube: R = Rz * Ry * Rx
        rx, ry, rz = (math.radians(a) for a in self.rot)
        cx, sx, cy, sy, cz, sz = math.cos(rx), math.sin(rx), math.cos(ry), math.sin(ry), math.cos(rz), math.sin(rz)
        Rx = [[1, 0, 0], [0, cx, -sx], [0, sx, cx]]
        Ry = [[cy, 0, sy], [0, 1, 0], [-sy, 0, cy]]
        Rz = [[cz, -sz, 0], [sz, cz, 0], [0, 0, 1]]
        return _matmul(Rz, _matmul(Ry, Rx))

    def corners(self):
        out = []
        for x in (self.frm[0], self.to[0]):
            for y in (self.frm[1], self.to[1]):
                for z in (self.frm[2], self.to[2]):
                    out.append(self.to_world([x, y, z]))
        return out

    def _bounds(self):
        cs = self.corners() if any(self.rot) else [self.frm, self.to]
        return [min(c[i] for c in cs) for i in range(3)], [max(c[i] for c in cs) for i in range(3)]

    def to_world(self, p):
        m = self._m
        q = [p[i] - self.origin[i] for i in range(3)]
        return [sum(m[i][j] * q[j] for j in range(3)) + self.origin[i] for i in range(3)]

    def dir_to_world(self, d):
        m = self._m
        return [sum(m[i][j] * d[j] for j in range(3)) for i in range(3)]

    def contains(self, p, eps=1e-4):
        lo, hi = self._aabb
        if not all(lo[i] - eps < p[i] < hi[i] + eps for i in range(3)):
            return False
        if not any(self.rot):
            return all(self.frm[i] + eps < p[i] < self.to[i] - eps for i in range(3))
        m = self._m
        q = [p[i] - self.origin[i] for i in range(3)]
        local = [sum(m[j][i] * q[j] for j in range(3)) + self.origin[i] for i in range(3)]
        return all(self.frm[i] + eps < local[i] < self.to[i] - eps for i in range(3))


def pair(box):
    """A box on the right (+x) side and its mirror image on the left."""
    return [box, box.mirrored()]


class Model:
    """A vehicle: its boxes, painters, preview livery and what Arnis needs to know.

    `bones` maps a moving bone's name to its pivot (wheels turn about x through it).
    `eye` is where a rider's eyes are (units, vehicle frame), checked to lie inside the
    glasshouse with nothing drawn in front of them but the outside of the vehicle."""

    def __init__(self, id, boxes, painters, livery, bones=None, eye=None, notes=""):
        self.id = id
        self.boxes = boxes
        self.painters = painters
        self.livery = {s: livery.get(s, livery.get("body")) for s in SLOTS}
        for s, block in self.livery.items():
            assert block in PAINTS, f"{id}: no paint for {block}"
        self.bones = bones or {}
        self.eye = eye
        self.notes = notes
        names = [b.name for b in boxes]
        dup = {n for n in names if names.count(n) > 1}
        assert not dup, f"{id}: box names repeat: {sorted(dup)}"
        for b in boxes:
            assert b.mat in painters, f"{id}: no painter {b.mat!r} for {b.name}"


# ---------------------------------------------------------------- faces

# For each face: its outward normal, the axis and direction u and v run along, and the
# plane it lies in, as Blockbench maps a face's UV rectangle onto it (reading upright
# from outside, Java's convention).
FACES = {
    "north": dict(normal=(0, 0, -1), u=(0, -1), v=(1, -1), plane=(2, 0)),
    "south": dict(normal=(0, 0, 1), u=(0, 1), v=(1, -1), plane=(2, 1)),
    "east": dict(normal=(1, 0, 0), u=(2, -1), v=(1, -1), plane=(0, 1)),
    "west": dict(normal=(-1, 0, 0), u=(2, 1), v=(1, -1), plane=(0, 0)),
    "up": dict(normal=(0, 1, 0), u=(0, 1), v=(2, 1), plane=(1, 1)),
    "down": dict(normal=(0, -1, 0), u=(0, 1), v=(2, -1), plane=(1, 0)),
}


def _texels(size):
    n = round(size)
    if abs(size - n) < 1e-6:
        return max(1, n)
    return max(1, math.ceil(size))


def _centres(size, n):
    if size < 1:
        return [size / 2]
    return [min(i + 0.5, (i + size) / 2) for i in range(n)]


def face_points(box, fname):
    f = FACES[fname]
    ua, us = f["u"]
    va, vs = f["v"]
    pa, pside = f["plane"]
    size_u = box.to[ua] - box.frm[ua]
    size_v = box.to[va] - box.frm[va]
    nu, nv = _texels(size_u), _texels(size_v)
    cu, cv = _centres(size_u, nu), _centres(size_v, nv)
    grid = []
    for j in range(nv):
        row = []
        for i in range(nu):
            local = [0.0, 0.0, 0.0]
            local[pa] = box.to[pa] if pside else box.frm[pa]
            local[ua] = box.frm[ua] + cu[i] if us > 0 else box.to[ua] - cu[i]
            local[va] = box.frm[va] + cv[j] if vs > 0 else box.to[va] - cv[j]
            row.append(local)
        grid.append(row)
    return size_u, size_v, nu, nv, grid


def hidden(box, fname, grid, boxes):
    """Whether every texel of a face sits inside some other box."""
    n_local = FACES[fname]["normal"]
    others = [o for o in boxes if o is not box]
    last = None
    for row in grid:
        for local in row:
            probe = box.to_world([local[i] + 0.02 * n_local[i] for i in range(3)])
            if last is not None and last.contains(probe):
                continue
            for o in others:
                if o.contains(probe):
                    last = o
                    break
            else:
                return False
    return True


class Ctx:
    """What a painter is told about a texel."""
    __slots__ = ("box", "face", "p", "n", "loc", "i", "j")

    def __init__(self, box, face, p, n, loc, i, j):
        self.box, self.face, self.p, self.n, self.loc, self.i, self.j = box, face, p, n, loc, i, j

    @property
    def x(self):
        return self.p[0]

    @property
    def y(self):
        return self.p[1]

    @property
    def z(self):
        return self.p[2]

    @property
    def side(self):
        return abs(self.n[0]) > 0.5

    @property
    def top(self):
        return self.n[1] > 0.5

    @property
    def bottom(self):
        return self.n[1] < -0.5

    @property
    def front(self):
        return self.n[2] < -0.5

    @property
    def back(self):
        return self.n[2] > 0.5


# Texel kinds, in the mask's blue channel.
LIT = 1        # drawn emissive on Bedrock (alpha 0 in its texture)
DIM_BRAKE = 2  # the brake light while driving on
BRIGHT_BRAKE = 4


def dim(c):
    return tuple(v * 9 // 16 for v in c)


class Face:
    __slots__ = ("box", "name", "su", "sv", "rgb", "mask", "variant")

    def __init__(self, box, name, su, sv, rgb, mask, variant):
        self.box, self.name, self.su, self.sv = box, name, su, sv
        self.rgb, self.mask, self.variant = rgb, mask, variant


def paint_faces(model):
    """Every drawn face of every box, painted in the preview livery, with its mask.
    Brake lamps are painted twice: dim (`variant` "dim") and bright ("bright")."""
    faces = []
    for box in model.boxes:
        painter = model.painters[box.mat]
        variants = ("dim", "bright") if box.lamp == "brake" else ("plain",)
        for fname in FACES:
            su, sv, nu, nv, grid = face_points(box, fname)
            if hidden(box, fname, grid, model.boxes):
                continue
            normal = box.dir_to_world(FACES[fname]["normal"])
            texels = []
            for j, row in enumerate(grid):
                for i, local in enumerate(row):
                    texels.append((i, j, painter(Ctx(box, fname, box.to_world(local), normal, local, i, j))))
            for variant in variants:
                rgb = Image.new("RGB", (nu, nv))
                mask = Image.new("RGBA", (nu, nv), (0, 0, 0, 255))
                flags = 0
                if box.lamp in ("head", "glow") or variant == "bright":
                    flags |= LIT
                if variant == "dim":
                    flags |= DIM_BRAKE
                if variant == "bright":
                    flags |= BRIGHT_BRAKE
                for i, j, out in texels:
                    if isinstance(out, Paint):
                        c = out.rgb(model.livery)
                        slot = SLOTS.index(out.slot) + 1
                        k = round((out.k + 1) * 127.5)
                    else:
                        c, slot, k = tuple(out), 0, 128
                    if variant == "dim":
                        c = dim(c)
                    rgb.putpixel((i, j), c)
                    mask.putpixel((i, j), (slot, k, flags, 255))
                faces.append(Face(box, fname, su, sv, rgb, mask, variant))
    return faces


def _shelves(faces, order, width):
    """Shelf-packs at `width`: each face's spot (one for identical faces) and the height."""
    spots, seen = {}, {}
    x = y = shelf = 0
    for k in order:
        f = faces[k]
        key = (f.rgb.size, hashlib.sha1(f.rgb.tobytes() + f.mask.tobytes()).hexdigest())
        if key in seen:
            spots[k] = seen[key]
            continue
        if x + f.rgb.width > width:
            x, y, shelf = 0, y + shelf, 0
        spots[k] = seen[key] = (x, y)
        x += f.rgb.width
        shelf = max(shelf, f.rgb.height)
    return spots, y + shelf


def pack(faces):
    """Packs the face images into the smallest texture, one spot for identical ones,
    both sides powers of two and no taller than wide (Blockbench reads a taller one as
    animation frames). Returns the texture in the preview livery, the mask and spots."""
    order = sorted(range(len(faces)), key=lambda k: (-faces[k].rgb.height, -faces[k].rgb.width))
    widest = max(f.rgb.width for f in faces)
    best = None
    width = 16
    while width < widest:
        width *= 2
    for _ in range(6):
        spots, used = _shelves(faces, order, width)
        height = max(16, 1 << math.ceil(math.log2(max(1, used))))
        if height <= width and (best is None or width * height < best[0] * best[1]):
            best = (width, height, spots)
        width *= 2
    if best is None:
        raise ValueError("faces do not fit a texture no taller than wide")
    width, height, spots = best
    tex = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    mask = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    for k, (sx, sy) in spots.items():
        tex.paste(faces[k].rgb, (sx, sy))
        mask.paste(faces[k].mask, (sx, sy))
    return tex, mask, spots


def png(img):
    buf = io.BytesIO()
    img.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def r4(v):
    v = round(v, 4)
    return 0.0 if v == 0 else v


# ---------------------------------------------------------------- Blockbench


def uid(*parts):
    return str(uuid.uuid5(uuid.NAMESPACE_URL, "arnis-vehicles/" + "/".join(parts)))


def bb_elements(model, faces, spots, texture_index, offset=(0, 0, 0), prefix=""):
    """The model's cubes as Blockbench elements, brake lamps in their dim form."""
    by_box = {}
    for k, f in enumerate(faces):
        if f.variant == "bright":
            continue
        u, v = spots[k]
        by_box.setdefault(id(f.box), {})[f.name] = {
            "uv": [r4(u), r4(v), r4(u + f.su), r4(v + f.sv)],
            "texture": texture_index,
        }
    elements, groups = [], {}
    for box in model.boxes:
        faces_of = {n: by_box.get(id(box), {}).get(n, {"uv": [0, 0, 0, 0], "texture": None}) for n in FACES}
        el = {
            "name": prefix + box.name,
            "box_uv": False,
            "rescale": False,
            "locked": False,
            "light_emission": 15 if box.lamp in ("head", "glow") else 0,
            "render_order": "default",
            "allow_mirror_modeling": True,
            "from": [r4(box.frm[i] + offset[i]) for i in range(3)],
            "to": [r4(box.to[i] + offset[i]) for i in range(3)],
            "autouv": 0,
            "color": 0,
            "origin": [r4(box.origin[i] + offset[i]) for i in range(3)],
            "faces": faces_of,
            "type": "cube",
            "uuid": uid(model.id, prefix, box.name),
        }
        if any(box.rot):
            el["rotation"] = [r4(a) for a in box.rot]
        elements.append(el)
        groups.setdefault(box.bone, []).append(el["uuid"])
    children = []
    for bone, members in groups.items():
        pivot = model.bones.get(bone, (0, 0, 0))
        children.append({
            "name": bone,
            "origin": [r4(pivot[i] + offset[i]) for i in range(3)],
            "color": 0, "uuid": uid(model.id, prefix, "group", bone), "export": True,
            "mirror_uv": False, "isOpen": False, "locked": False, "visibility": True,
            "autouv": 0, "children": members,
        })
    root = {
        "name": model.id, "origin": [r4(v) for v in offset], "color": 0,
        "uuid": uid(model.id, prefix, "root"), "export": True, "mirror_uv": False,
        "isOpen": False, "locked": False, "visibility": True, "autouv": 0, "children": children,
    }
    return elements, root


def bb_texture(name, img, index, uv_size=None):
    w, h = img.size
    return {
        "path": "", "name": name, "folder": "", "namespace": "", "id": str(index), "group": "",
        "width": w, "height": h, "uv_width": (uv_size or (w, h))[0], "uv_height": (uv_size or (w, h))[1],
        "particle": False, "use_as_default": False, "layers_enabled": False, "sync_to_project": "",
        "render_mode": "default", "render_sides": "auto", "frame_time": 1, "frame_order_type": "loop",
        "frame_order": "", "frame_interpolate": False, "visible": True, "internal": True, "saved": False,
        "uuid": uid("texture", name), "source": "data:image/png;base64," + base64.b64encode(png(img)).decode(),
    }


def bbmodel(name, parts, free=False):
    """A Blockbench project of one or more built models: parts are (built, offset)."""
    elements, outliner, textures = [], [], []
    per_texture = free  # several textures of their own sizes need the free format
    for i, (b, offset) in enumerate(parts):
        els, root = bb_elements(b.model, b.faces, b.spots, i, offset, prefix=(b.model.id + "/") if len(parts) > 1 else "")
        elements += els
        outliner.append(root)
        textures.append(bb_texture(b.model.id + ".png", b.texture.convert("RGBA"), i))
    w = max(t["width"] for t in textures)
    h = max(t["height"] for t in textures)
    return {
        "meta": {"format_version": "5.0", "model_format": "free" if per_texture else "bedrock", "box_uv": False},
        "name": name,
        "model_identifier": name,
        "visible_box": [64, 32, 0],
        "variable_placeholders": "", "variable_placeholder_buttons": [], "timeline_setups": [],
        "unhandled_root_fields": {},
        "resolution": {"width": w, "height": h},
        "elements": elements,
        "outliner": outliner,
        "textures": textures,
    }


# ---------------------------------------------------------------- Bedrock


def bedrock_geometry(built):
    """Bedrock geometry in units at 1:1; Arnis scales it to the world. Head and glow
    lamps go into the `lit` bone, brake lamps into `brake_off` (dim) and `lit_brake`."""
    model, faces, spots = built.model, built.faces, built.spots
    uv_of = {}
    for k, f in enumerate(faces):
        u, v = spots[k]
        w, h = f.su, f.sv
        entry = {"uv": [r4(u), r4(v)], "uv_size": [r4(w), r4(h)]}
        if f.name in ("up", "down"):
            entry = {"uv": [r4(u + w), r4(v + h)], "uv_size": [r4(-w), r4(-h)]}
        uv_of.setdefault((id(f.box), f.variant), {})[f.name] = entry

    def cube(box, variant):
        uv = uv_of.get((id(box), variant))
        if not uv:
            return None
        size = [box.to[i] - box.frm[i] for i in range(3)]
        out = {
            "origin": [r4(-(box.frm[0] + size[0])), r4(box.frm[1]), r4(box.frm[2])],
            "size": [r4(s) for s in size],
            "uv": uv,
        }
        if any(box.rot):
            out["pivot"] = [r4(-box.origin[0]), r4(box.origin[1]), r4(box.origin[2])]
            out["rotation"] = [r4(-box.rot[0]), r4(-box.rot[1]), r4(box.rot[2])]
        return out

    bones = {}
    order = []

    def put(bone, c):
        if c is None:
            return
        if bone not in bones:
            bones[bone] = []
            order.append(bone)
        bones[bone].append(c)

    for box in model.boxes:
        if box.lamp in ("head", "glow"):
            put("lit" if box.bone == "body" else "lit_" + box.bone, cube(box, "plain"))
        elif box.lamp == "brake":
            put("brake_off", cube(box, "dim"))
            put("lit_brake", cube(box, "bright"))
        else:
            put(box.bone, cube(box, "plain"))
    lo, hi = built.bounds
    width = 2 * max(abs(lo[0]), abs(hi[0]), abs(lo[2]), abs(hi[2])) / 16 + 1
    height = hi[1] / 16 + 1
    return {
        "format_version": "1.12.0",
        "minecraft:geometry": [{
            "description": {
                "identifier": f"geometry.arnis.{model.id}",
                "texture_width": built.texture.width,
                "texture_height": built.texture.height,
                "visible_bounds_width": math.ceil(width),
                "visible_bounds_height": math.ceil(height),
                "visible_bounds_offset": [0, r4(height / 2), 0],
            },
            "bones": [
                {
                    "name": b,
                    "pivot": [r4(-model.bones.get(b, (0, 0, 0))[0]), r4(model.bones.get(b, (0, 0, 0))[1]), r4(model.bones.get(b, (0, 0, 0))[2])],
                    "cubes": bones[b],
                }
                for b in order
            ],
        }],
    }


# ---------------------------------------------------------------- Java

JAVA_LIMIT = (-16.0, 32.0)


def java_frame(built):
    """How the model is fitted into Java's -16..32: units -> (v - centre) * scale + 8."""
    boxes = built.model.boxes
    lo = [min(min(b.frm[i], b.origin[i]) for b in boxes) for i in range(3)]
    hi = [max(max(b.to[i], b.origin[i]) for b in boxes) for i in range(3)]
    centre = [(lo[i] + hi[i]) / 2 for i in range(3)]
    reach = max(max(hi[i] - centre[i], centre[i] - lo[i]) for i in range(3))
    scale = min(1.0, 23.9 / reach) if reach > 0 else 1.0
    return centre, scale


def java_models(built, texture_ref):
    """The Java item models in the fitted frame: the body, with head lamps glowing; the
    brake lamps on their own, bright (None without any); and the car parked, its body and
    dim brake lamps, nothing glowing (None without brake lamps)."""
    model, faces, spots = built.model, built.faces, built.spots
    tw, th = built.texture.size
    centre, scale = java_frame(built)
    to_m = lambda v, i: r4((v - centre[i]) * scale + 8)
    face_of = {}
    for k, f in enumerate(faces):
        u, v = spots[k]
        face_of.setdefault((id(f.box), f.variant), {})[f.name] = {
            "uv": [r4(u * 16 / tw), r4(v * 16 / th), r4((u + f.su) * 16 / tw), r4((v + f.sv) * 16 / th)],
            "texture": "#t",
        }

    def element(box, variant):
        fs = face_of.get((id(box), variant))
        if not fs:
            return None
        el = {
            "from": [to_m(box.frm[i], i) for i in range(3)],
            "to": [to_m(box.to[i], i) for i in range(3)],
            "faces": fs,
        }
        for i in range(3):
            assert JAVA_LIMIT[0] - 1e-6 <= el["from"][i] and el["to"][i] <= JAVA_LIMIT[1] + 1e-6, (model.id, box.name, el)
        axis = next((i for i in range(3) if box.rot[i]), None)
        if axis is not None:
            el["rotation"] = {
                "angle": r4(box.rot[axis]),
                "axis": "xyz"[axis],
                "origin": [to_m(box.origin[i], i) for i in range(3)],
            }
        if box.lamp in ("head", "glow"):
            el["light_emission"] = 15
        return el

    body, tail, dim = [], [], []
    for box in model.boxes:
        if box.lamp == "brake":
            e = element(box, "bright")
            if e:
                tail.append(e)
            e = element(box, "dim")
            if e:
                dim.append(e)
        else:
            e = element(box, "plain")
            if e:
                body.append(e)
    textures = {"t": texture_ref, "particle": texture_ref}
    main = {"textures": textures, "elements": body}
    dark = [{k: v for k, v in e.items() if k != "light_emission"} for e in body + dim]
    parked = {"textures": textures, "elements": dark} if tail else None
    return main, ({"textures": textures, "elements": tail} if tail else None), parked, {"centre": [r4(c) for c in centre], "scale": scale}


def squeeze(faces):
    """Shrinks each face's image to one texel along an axis it does not change along
    (a hull side shaded only by height, a deck striped only lengthwise), colour and mask
    alike: the game stretches that texel across the face, so it looks the same and takes
    a sliver of the texture."""
    for f in faces:
        for axis in (0, 1):
            w, h = f.rgb.size
            n = (w, h)[axis]
            if n < 2:
                continue
            first = (0, 0, 1, h) if axis == 0 else (0, 0, w, 1)
            line_rgb, line_mask = f.rgb.crop(first), f.mask.crop(first)
            same = all(
                f.rgb.crop(box).tobytes() == line_rgb.tobytes()
                and f.mask.crop(box).tobytes() == line_mask.tobytes()
                for box in (
                    ((k, 0, k + 1, h) if axis == 0 else (0, k, w, k + 1)) for k in range(1, n)
                )
            )
            if same:
                f.rgb, f.mask = line_rgb, line_mask
                if axis == 0:
                    f.su = 1.0
                else:
                    f.sv = 1.0


# ---------------------------------------------------------------- building


class Built:
    def __init__(self, model):
        self.model = model
        self.faces = paint_faces(model)
        assert self.faces, f"{model.id}: nothing to draw"
        squeeze(self.faces)
        self.texture, self.mask, self.spots = pack(self.faces)
        lo = [min(b._aabb[0][i] for b in model.boxes) for i in range(3)]
        hi = [max(b._aabb[1][i] for b in model.boxes) for i in range(3)]
        self.bounds = (lo, hi)

    def stats(self):
        drawn = sum(1 for f in self.faces if f.variant != "bright")
        return {
            "boxes": len(self.model.boxes),
            "quads": drawn,
            "texture": list(self.texture.size),
            "size_m": [r4((self.bounds[1][i] - self.bounds[0][i]) / 16) for i in range(3)],
            "bounds": [[r4(v) for v in self.bounds[0]], [r4(v) for v in self.bounds[1]]],
        }


def write(built, out_dir):
    """Writes the model's files: Blockbench project, textures, Bedrock geometry, Java
    models and a summary Arnis reads."""
    m = built.model
    os.makedirs(out_dir, exist_ok=True)
    base = os.path.join(out_dir, m.id)
    with open(base + ".bbmodel", "w") as f:
        json.dump(bbmodel(m.id, [(built, (0, 0, 0))]), f, separators=(",", ":"))
    built.texture.convert("RGB").save(base + ".png", optimize=True)
    built.mask.save(base + ".mask.png", optimize=True)
    with open(base + ".geo.json", "w") as f:
        json.dump(bedrock_geometry(built), f, separators=(",", ":"))
    main, tail, parked, frame = java_models(built, f"arnis:block/vehicle/{m.id}")
    with open(base + ".java.json", "w") as f:
        json.dump(main, f, separators=(",", ":"))
    if tail:
        with open(base + "_brake.java.json", "w") as f:
            json.dump(tail, f, separators=(",", ":"))
        with open(base + "_parked.java.json", "w") as f:
            json.dump(parked, f, separators=(",", ":"))
    meta = {"id": m.id, "java": frame, "brake": tail is not None, "bones": {k: [r4(v) for v in p] for k, p in m.bones.items()}}
    meta.update(built.stats())
    with open(base + ".meta.json", "w") as f:
        json.dump(meta, f, indent=1)
    return meta
