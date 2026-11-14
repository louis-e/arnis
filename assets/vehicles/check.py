#!/usr/bin/env python3
"""Checks every model (or the given ones) for the two faults a render does not show well:

- faces that flicker: two drawn faces facing the same way, less than 0.25 units apart
  and overlapping;
- a blocked view: what a rider at `Model.eye` sees around the horizon (pitch -5..+10
  degrees, all round), by the first face each ray meets. Faces seen from behind are not
  drawn, as in the game, so the shell does not count; anything listed stands in the view.

    python check.py [id ...]

Needs numpy.
"""
import math
import sys

import numpy as np

import kit
import preview
from build import models

GAP = 0.25


def covered(built, skip, points):
    """Whether every point lies inside a box other than those in `skip`."""
    boxes = [b for b in built.model.boxes if id(b) not in skip]
    return all(any(b.contains(list(p)) for b in boxes) for p in points)


def flicker(built):
    """Pairs of drawn faces that face the same way less than GAP apart and overlap where
    no third box covers them."""
    qs, _ = preview.quads(built)
    drawn = [f for f in built.faces if f.variant != "bright"]
    names = [f"{f.box.name}.{f.name}" for f in drawn]
    O = np.array([q[0] for q in qs])
    U = np.array([q[1] for q in qs])
    V = np.array([q[2] for q in qs])
    N = np.array([q[3] for q in qs])
    corners = np.stack([O, O + U, O + V, O + U + V], axis=1)
    bad = []
    for i in range(len(qs)):
        same = (N[i + 1:] @ N[i]) > 0.999
        near = np.abs((O[i + 1:] - O[i]) @ N[i]) < GAP
        for j in np.nonzero(same & near)[0] + i + 1:
            area, spans = 1.0, []
            for axis in (U[i], V[i]):
                ln = np.linalg.norm(axis)
                if ln < 1e-9:
                    area = 0.0
                    break
                a = corners[i] @ axis / ln
                b = corners[j] @ axis / ln
                lo, hi = max(a.min(), b.min()), min(a.max(), b.max())
                area *= max(0.0, hi - lo)
                spans.append((axis / ln, lo, hi))
            if area <= 1e-3:
                continue
            # Sample the overlap just in front of the nearer face.
            front = max(O[i] @ N[i], O[j] @ N[i]) + 0.05
            base = N[i] * (front - O[i] @ N[i]) + O[i] - sum(
                ax * (O[i] @ ax) for ax, _, _ in spans)
            pts = [base + spans[0][0] * (spans[0][1] + (spans[0][2] - spans[0][1]) * fu)
                   + spans[1][0] * (spans[1][1] + (spans[1][2] - spans[1][1]) * fv)
                   for fu in (0.05, 0.5, 0.95) for fv in (0.05, 0.5, 0.95)]
            if covered(built, {id(drawn[i].box), id(drawn[j].box)}, pts):
                continue
            gap = abs((O[j] - O[i]) @ N[i])
            bad.append((names[i], names[j], f"area {area:.1f}", f"apart {gap:.2f}"))
    return bad


def view(built, eye):
    """{box name: (rays that meet it first, nearest distance)} around the horizon."""
    qs, _ = preview.quads(built)
    boxes = [f.box.name for f in built.faces if f.variant != "bright"]
    O = np.array([q[0] for q in qs])
    U = np.array([q[1] for q in qs])
    V = np.array([q[2] for q in qs])
    N = np.array([q[3] for q in qs])
    E = np.array(eye, float)
    seen, rays = {}, 0
    for pitch in range(-5, 11, 5):
        for yaw in range(0, 360, 5):
            p, y = math.radians(pitch), math.radians(yaw)
            D = np.array([math.sin(y) * math.cos(p), math.sin(p), -math.cos(y) * math.cos(p)])
            rays += 1
            facing = (N @ D) < -1e-9
            # Solve O + sU + tV = E + dD for every quad facing the ray at once.
            M = np.stack([U, V, -np.broadcast_to(D, U.shape)], axis=2)
            ok = facing & (np.abs(np.linalg.det(M)) > 1e-12)
            if not ok.any():
                continue
            sol = np.linalg.solve(M[ok], (E - O[ok])[..., None])[..., 0]
            s, t, d = sol[:, 0], sol[:, 1], sol[:, 2]
            hit = (s >= 0) & (s <= 1) & (t >= 0) & (t <= 1) & (d > 0.01)
            if not hit.any():
                continue
            idx = np.nonzero(ok)[0][hit]
            k = int(np.argmin(d[hit]))
            name, dist = boxes[idx[k]], float(d[hit][k])
            n, near = seen.get(name, (0, 1e9))
            seen[name] = (n + 1, min(near, dist))
    return seen, rays


def main(argv):
    status = 0
    for m in models(strict=True):
        if argv and m.id not in argv:
            continue
        b = kit.Built(m)
        bad = flicker(b)
        print(f"{m.id}: {len(bad)} flickering pairs")
        for x in bad[:12]:
            print("    flicker", *x)
        status |= bool(bad)
        if m.eye is not None:
            seen, rays = view(b, m.eye)
            blocked = sum(n for n, _ in seen.values())
            print(f"    rider at {tuple(round(v / 16, 2) for v in m.eye)} m sees parts of the "
                  f"vehicle in {blocked} of {rays} rays round the horizon")
            for name, (n, near) in sorted(seen.items(), key=lambda kv: -kv[1][0])[:8]:
                print(f"      {name}: {n} rays, nearest {near / 16:.2f} m")
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
