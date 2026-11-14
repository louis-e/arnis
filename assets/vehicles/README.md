# Vehicle models

The cars, trains and ships of `--moving-vehicles` (and the parked cars with it), drawn as
real models instead of stretched blocks: an `item_display` with a model from the world's
resource pack on Java (1.21.6+), an entity geometry on Bedrock. The signal heads of
`--traffic-lights` are built the same way (`models/signals.py`, one model per state the
data pack switches between); being small, they are drawn at 32 units to the metre, and
with no box turned they work from Java 1.21.4, as the traffic lights always have.

Each model is Python in `models/`, built with `kit.py`. `build.py` writes for every model,
into `out/`:

| file | for |
|---|---|
| `<id>.bbmodel` | Blockbench, to look at or edit |
| `<id>.png` | the texture in the preview livery |
| `<id>.mask.png` | per texel: livery slot (R, 0 = fixed colour), shade (G, 128 = none), lamp flags (B) |
| `<id>.geo.json` | Bedrock geometry at 1:1, lamp bones `lit`, `brake_off`, `lit_brake`, wheel bones |
| `<id>.java.json`, `<id>_brake.java.json` | Java item models, fitted into -16..32 |
| `<id>.meta.json` | how the Java model was fitted, bone pivots, counts |

and `review.bbmodel`, every model side by side. Arnis embeds `out/` and colours each model
for its livery from the mask, so `out/` is committed, all but the `.bbmodel` files, which
are only for looking at. Rebuild after changing a model:

```sh
python build.py              # everything, and review.bbmodel
python build.py car_sedan    # one model (or several)
python preview.py car_sedan /tmp/sedan.png [--zoom 3] [--views front34,side,top]
python check.py [car_sedan ...]   # flickering faces, and what blocks a rider's view
```

`build.py` needs Pillow, `preview.py` and `check.py` numpy as well. Run `check.py` before
committing a model: it should report no flickering pairs, and from the rider's seat only
parts outside the cabin (mirrors, the bonnet, a ship's superstructure), none a few
centimetres away.

## Conventions

- **Units.** 16 to a metre, one texel per unit, Minecraft's density. Vehicles are 1:1;
  Arnis scales them to the world.
- **Frame.** Front towards **-z**, the vehicle's right side at **+x**, the road, rail or
  waterline at **y = 0**, centred on x = 0 and on z = 0 lengthwise.
- **Boxes.** `Box(name, frm, to, painter, bone="body", origin=..., rot=..., lamp=...)`.
  A box may turn about one axis, by at most 45 degrees (Java takes any angle in that range
  from 1.21.6). Anything steeper is an upright slab leaned the other way, as the
  hatchback's rear window. `pair(box)` adds the mirror image on the left;
  `box.moved(dz=...)` repeats one along the body.
- **Painters.** `painter(c)` gets a `Ctx`: `c.p` the spot in the vehicle frame, `c.n` the
  face's normal, `c.loc` the spot before the box was turned, `c.face`, and `c.side`,
  `c.top`, `c.front` (facing -z), `c.back`, `c.bottom`. It returns an RGB tuple for a fixed
  colour (glass, rubber, metal, lamps) or `Paint(slot, k)` for the livery: slots `body`,
  `lower`, `stripe`, `door`, `roof` as in `train_bodies.rs`' `Livery`, `k` from -1 (black)
  to 1 (white). Everything in an operator's or owner's colours must be `Paint`.
- **Turned faces** have a turned texel grid: only lines along the face's own axes stay
  clean, so paint those from `c.loc`, and keep level stripes off them.
- **Lamps.** `lamp="head"` always lit, `"glow"` other lit things (destination displays,
  marker lights), `"brake"` a car's tail lights (dim, bright while braking; drawn by a
  separate Java display).
- **Bones.** A road wheel goes in its own bone, named `wheel_*`, pivot at its centre in
  `Model(bones=...)`; Bedrock turns those with speed.
- **Hidden faces** (inside another box) are left out by themselves. Two coplanar faces
  over each other flicker: keep a part standing proud by at least 0.25 units, or keep the
  two from overlapping.
- **The rider.** Riders sit with their eyes at `Model(eye=...)`, inside the vehicle. A
  box's own faces are not drawn from inside it, so the shell does not block the view, but
  the outside of any box standing inside the cabin does: no seats, walls or dashboards
  above the knees in front of a rider.
- **Glass** is opaque, tinted, with short glints of sky (`kit.glass`). No transparency.
- **Long bodies** are repeated bays or segments: faces that come out identical share one
  spot of the texture, so a 25 m carriage costs little more texture than one bay.

## Budgets

Every model is drawn every frame for each vehicle in view, and its texture goes into the
block atlas the facade photos share.

| | boxes | quads | texture |
|---|---|---|---|
| cars | 120 | 450 | 256² |
| buses, trucks | 160 | 600 | 256² |
| carriages, locomotives | 200 | 700 | 512² |
| freight wagons | 80 | 300 | 256² |
| boats up to 40 m | 150 | 500 | 512² |
| ships | 300 | 1200 | 1024² |

## Style

`models/car_hatchback.py` sets the bar: Minecraft-blocky but detailed, pixel-art shading
(a lighter upper edge, darker lower parts, 1-texel panel gaps, crisp lamps, glints on
glass), real proportions and the details that make the type recognisable. No brand marks.
