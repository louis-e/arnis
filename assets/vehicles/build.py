#!/usr/bin/env python3
"""Builds every vehicle model into out/ and all of them side by side into review.bbmodel.

    python build.py [id ...]          only these models (out/ keeps the others)
    python build.py --review-only     just review.bbmodel

Needs Pillow. Each module in models/ lists its builders in MODELS; a builder returns a
Model or a list of them.
"""
import importlib
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import kit  # noqa: E402

OUT = os.path.join(HERE, "out")


def models(strict=False):
    """Every model the modules in models/ build. A module that fails is reported and left
    out, so one half-finished file does not stop work on the others, unless `strict`."""
    found = []
    folder = os.path.join(HERE, "models")
    for name in sorted(os.listdir(folder)):
        if not name.endswith(".py") or name.startswith("_"):
            continue
        try:
            module = importlib.import_module("models." + name[:-3])
            for builder in module.MODELS:
                made = builder()
                found += made if isinstance(made, list) else [made]
        except Exception as e:
            if strict:
                raise
            print(f"skipped models/{name}: {type(e).__name__}: {e}", file=sys.stderr)
    ids = [m.id for m in found]
    assert len(ids) == len(set(ids)), "model ids repeat"
    return found


def review(built, path):
    """All models in rows by family (cars, trains, ships), a few metres apart."""
    rows = {}
    for b in built:
        rows.setdefault(b.model.id.split("_")[0], []).append(b)
    parts, z = [], 0.0
    for family in ("car", "train", "freight", "ship"):
        row = rows.pop(family, [])
        if not row:
            continue
        x, depth = 0.0, 0.0
        for b in row:
            lo, hi = b.bounds
            x += -lo[0]
            parts.append((b, (x, 0.0, z - lo[2])))
            x += hi[0] + 24
            depth = max(depth, hi[2] - lo[2])
        z += depth + 48
    for row in rows.values():
        x = 0.0
        for b in row:
            lo, hi = b.bounds
            parts.append((b, (x - lo[0], 0.0, z - lo[2])))
            x += hi[0] - lo[0] + 24
    with open(path, "w") as f:
        json.dump(kit.bbmodel("arnis_vehicles", parts, free=True), f, separators=(",", ":"))


def main(argv):
    only = [a for a in argv if not a.startswith("--")]
    all_models = models(strict=not only)
    chosen = [m for m in all_models if not only or m.id in only]
    missing = set(only) - {m.id for m in chosen}
    if missing:
        sys.exit(f"no such model: {', '.join(sorted(missing))}")
    built = []
    for m in all_models:
        if m in chosen or "--review-only" in argv or not only:
            b = kit.Built(m)
            built.append(b)
            if m in chosen and "--review-only" not in argv:
                meta = kit.write(b, OUT)
                print(f"{m.id}: {meta['boxes']} boxes, {meta['quads']} quads, texture "
                      f"{meta['texture'][0]}x{meta['texture'][1]}, {meta['size_m']} m")
    if not only or "--review-only" in argv:
        review(built, os.path.join(HERE, "review.bbmodel"))
        print(f"review.bbmodel: {len(built)} models")


if __name__ == "__main__":
    main(sys.argv[1:])
