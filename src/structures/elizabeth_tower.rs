//! The Elizabeth Tower (Big Ben) at one block per metre, staged at its mapped parts' heights.

use std::collections::BTreeMap;

use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;
use crate::element_processing::structure_kit::top_slab;

/// Model grid side; the tower is centred on block `C`.
const SIDE: i32 = 17;
const C: i32 = 8;

/// Dial centre layer.
const DIAL_Y: i32 = 54;
/// Layers of the string courses between the shaft's tiers of panels.
const BANDS: [i32; 6] = [12, 19, 26, 33, 40, 46];
/// Layer and half-width of the roof from the belfry up to the lantern.
const ROOF: [(i32, i32); 7] = [
    (66, 5),
    (67, 5),
    (68, 4),
    (69, 4),
    (70, 4),
    (71, 3),
    (72, 3),
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Part {
    Block(Block),
    /// A stair raised toward the tower's axis; `top` hangs it upside down.
    Stair {
        block: Block,
        face: usize,
        top: bool,
    },
    SlabTop(Block),
}

/// Viewer-right and outward directions of the north, east, south and west faces.
const FACES: [((i32, i32), (i32, i32)); 4] = [
    ((-1, 0), (0, -1)),
    ((0, -1), (1, 0)),
    ((1, 0), (0, 1)),
    ((0, 1), (-1, 0)),
];

#[derive(Default)]
struct Tower {
    cells: BTreeMap<(i32, i32, i32), Part>,
}

impl Tower {
    fn set(&mut self, x: i32, y: i32, z: i32, part: Part) {
        self.cells.insert((x, y, z), part);
    }

    /// Face `f` at `u` blocks right of centre and `d` blocks out from the axis.
    fn face(&mut self, f: usize, u: i32, y: i32, d: i32, part: Part) {
        let ((rx, rz), (nx, nz)) = FACES[f];
        self.set(C + rx * u + nx * d, y, C + rz * u + nz * d, part);
    }

    fn clear_face(&mut self, f: usize, u: i32, y: i32, d: i32) {
        let ((rx, rz), (nx, nz)) = FACES[f];
        self.cells
            .remove(&(C + rx * u + nx * d, y, C + rz * u + nz * d));
    }

    /// Every cell `h` out from the axis, through `pick(face, u)`.
    fn ring(&mut self, y: i32, h: i32, mut pick: impl FnMut(usize, i32) -> Option<Part>) {
        for f in 0..4 {
            for u in -h..=h {
                if let Some(part) = pick(f, u) {
                    self.face(f, u, y, h, part);
                }
            }
        }
    }

    fn fill(&mut self, y: i32, h: i32, part: Part) {
        for x in C - h..=C + h {
            for z in C - h..=C + h {
                self.set(x, y, z, part);
            }
        }
    }

    /// A pyramid layer of half-width `h`: stairs on its edges where the next layer steps in.
    fn roof_layer(&mut self, y: i32, h: i32, next: i32, tile: Block, stair: Block, hip: Part) {
        self.ring(y, h, |f, u| {
            Some(if u.abs() == h {
                hip
            } else if next < h {
                Part::Stair {
                    block: stair,
                    face: f,
                    top: false,
                }
            } else {
                Part::Block(tile)
            })
        });
    }
}

fn b(block: Block) -> Part {
    Part::Block(block)
}

fn stair(block: Block, face: usize, top: bool) -> Part {
    Part::Stair { block, face, top }
}

pub fn build() -> PalettizedSchematic {
    let mut t = Tower::default();
    plinth(&mut t);
    shaft(&mut t);
    clock_stage(&mut t);
    belfry(&mut t);
    roof(&mut t);
    interior(&mut t);
    finish(t)
}

fn plinth(t: &mut Tower) {
    t.fill(0, 7, b(SMOOTH_SANDSTONE));
    for y in 1..=4 {
        t.ring(y, 7, |_, u| {
            Some(b(if u.abs() == 7 || y == 4 {
                SANDSTONE
            } else if u % 2 != 0 {
                CUT_SANDSTONE
            } else {
                SMOOTH_SANDSTONE
            }))
        });
        t.ring(y, 6, |_, _| Some(b(SMOOTH_SANDSTONE)));
    }
    // Water table sloping off the plinth.
    t.ring(5, 7, |f, u| {
        Some(if u.abs() == 7 {
            b(CUT_SANDSTONE_SLAB)
        } else {
            stair(SMOOTH_SANDSTONE_STAIRS, f, false)
        })
    });
}

fn shaft(t: &mut Tower) {
    for y in 5..=46 {
        let band = BANDS.contains(&y);
        // Rows from the tier's band above: the panel heads, then three window rows.
        let below_band = BANDS.iter().find(|&&b| b >= y).map_or(0, |&b| b - y);
        for f in 0..4 {
            for u in -6..=6i32 {
                let panel = u % 2 != 0;
                let (front, back) = if band {
                    let carved = if panel {
                        CHISELED_SANDSTONE
                    } else {
                        SMOOTH_SANDSTONE
                    };
                    (Some(b(carved)), SMOOTH_SANDSTONE)
                } else if !panel {
                    let mullion = if u.abs() == 6 {
                        SANDSTONE
                    } else {
                        SMOOTH_SANDSTONE
                    };
                    (Some(b(mullion)), SMOOTH_SANDSTONE)
                } else {
                    // Recessed panels, hooded at the head, the inner four glazed.
                    let window = matches!(u.abs(), 1 | 3) && (2..=4).contains(&below_band);
                    (
                        (below_band == 1).then(|| stair(SMOOTH_SANDSTONE_STAIRS, f, true)),
                        if window {
                            BLACK_STAINED_GLASS
                        } else {
                            CUT_SANDSTONE
                        },
                    )
                };
                if let Some(front) = front {
                    t.face(f, u, y, 6, front);
                }
                // A corner's backing would land in the next face's front.
                if u.abs() < 6 {
                    t.face(f, u, y, 5, b(back));
                }
            }
        }
        // String courses stand out as a ledge.
        if band && y < 46 {
            t.ring(y, 7, |_, _| Some(b(CUT_SANDSTONE_SLAB)));
        }
    }
    // Clasping buttresses round the lower corners, capped where they stop.
    for y in 5..=19 {
        t.ring(y, 7, |f, u| {
            (u.abs() >= 5).then(|| {
                if y < 19 {
                    b(SANDSTONE)
                } else if u.abs() == 7 {
                    b(CUT_SANDSTONE_SLAB)
                } else {
                    stair(SMOOTH_SANDSTONE_STAIRS, f, false)
                }
            })
        });
    }
    // Corbels carrying the wider clock stage.
    t.ring(46, 7, |f, u| {
        Some(if u.abs() == 7 {
            Part::SlabTop(CUT_SANDSTONE_SLAB)
        } else {
            stair(SMOOTH_SANDSTONE_STAIRS, f, true)
        })
    });
}

/// The dial, its square and the inscription below, at `u` across and `dy` above centre.
fn dial(u: i32, dy: i32) -> (Part, Block) {
    let in_disc = |u: i32, dy: i32| f64::from(u * u + dy * dy) <= 3.45 * 3.45;
    if dy == -5 {
        return (
            b(if u % 2 == 0 {
                RAW_GOLD_BLOCK
            } else {
                LAPIS_BLOCK
            }),
            SANDSTONE,
        );
    }
    // A gilded frame round carved spandrels, each with a gilded rosette.
    if u.abs() == 4 || dy.abs() == 4 || (u.abs() == 3 && dy.abs() == 3) {
        return (b(RAW_GOLD_BLOCK), SANDSTONE);
    }
    if !in_disc(u, dy) {
        return (b(CHISELED_SANDSTONE), SANDSTONE);
    }
    let rim =
        !(in_disc(u + 1, dy) && in_disc(u - 1, dy) && in_disc(u, dy + 1) && in_disc(u, dy - 1));
    // Chapter ring, hub and hands in Prussian blue: minute hand at twelve, hour hand at nine.
    if rim {
        return (b(LAPIS_BLOCK), SANDSTONE);
    }
    if matches!((u, dy), (0, 0) | (0, 1) | (0, 2) | (-1, 0)) {
        return (b(LAPIS_BLOCK), SEA_LANTERN);
    }
    // Opal glass lit from behind, white by day.
    (b(WHITE_STAINED_GLASS), SEA_LANTERN)
}

fn clock_stage(t: &mut Tower) {
    for y in 47..=59 {
        let dy = y - DIAL_Y;
        for f in 0..4 {
            for u in -7..=7i32 {
                let (front, back) = if y == 59 && u.abs() <= 5 {
                    // Shields along a gilded band over the dial.
                    let shield = if u % 2 != 0 {
                        WHITE_CONCRETE
                    } else {
                        RAW_GOLD_BLOCK
                    };
                    (b(shield), SMOOTH_SANDSTONE)
                } else if y == 47 || y == 59 {
                    (b(SMOOTH_SANDSTONE), SMOOTH_SANDSTONE)
                } else if u.abs() == 7 {
                    (b(CHISELED_SANDSTONE), SANDSTONE)
                } else if u.abs() >= 5 || dy < -5 {
                    (b(CUT_SANDSTONE), SANDSTONE)
                } else {
                    dial(u, dy)
                };
                t.face(f, u, y, 7, front);
                if u.abs() < 7 {
                    t.face(f, u, y, 6, b(back));
                }
            }
        }
    }
    // Gilded cresting round the stage's top, pinnacles at its corners and over each dial's frame.
    t.ring(60, 7, |_, u| {
        Some(b(if u.abs() == 7 {
            SANDSTONE_WALL
        } else if u % 2 == 0 {
            RAW_GOLD_BLOCK
        } else {
            CUT_SANDSTONE_SLAB
        }))
    });
    for f in 0..4 {
        for y in 61..=62 {
            t.face(f, 7, y, 7, b(SANDSTONE_WALL));
        }
        t.face(f, 7, 63, 7, b(GOLD_BLOCK));
        for u in [-4, 4] {
            t.face(f, u, 61, 7, b(SANDSTONE_WALL));
            t.face(f, u, 62, 7, b(LIGHTNING_ROD));
        }
    }
}

fn belfry(t: &mut Tower) {
    for y in 60..=65 {
        for f in 0..4 {
            for u in -6..=6i32 {
                let lancet = u % 2 == 0 && u.abs() <= 4;
                let part = match y {
                    65 if u % 2 == 0 && u.abs() < 6 => b(RAW_GOLD_BLOCK),
                    60 | 65 => b(SMOOTH_SANDSTONE),
                    // Open lancets onto the bells, pointed by an overhang at their heads.
                    61..=63 if lancet => {
                        t.clear_face(f, u, y, 6);
                        continue;
                    }
                    64 if lancet => stair(SMOOTH_SANDSTONE_STAIRS, f, true),
                    _ if u.abs() == 6 => b(CHISELED_SANDSTONE),
                    _ => b(SANDSTONE),
                };
                t.face(f, u, y, 6, part);
            }
        }
    }
    t.ring(65, 7, |_, _| Some(b(CUT_SANDSTONE_SLAB)));
    // Corner pinnacles beside the roof.
    for f in 0..4 {
        for y in 66..=69 {
            t.face(f, 6, y, 6, b(SANDSTONE_WALL));
        }
        t.face(f, 6, 70, 6, b(GOLD_BLOCK));
    }
}

fn roof(t: &mut Tower) {
    // Cast-iron roof up to the lantern, with rows of small dormers.
    for (i, &(y, h)) in ROOF.iter().enumerate() {
        let next = ROOF.get(i + 1).map_or(3, |r| r.1);
        t.roof_layer(
            y,
            h,
            next,
            POLISHED_ANDESITE,
            POLISHED_ANDESITE_STAIRS,
            b(ANDESITE),
        );
    }
    for (y, h, us) in [(67, 5, &[-2, 0, 2][..]), (70, 4, &[0][..])] {
        for f in 0..4 {
            for &u in us {
                t.face(f, u, y, h, b(BLACK_STAINED_GLASS));
                t.face(f, u, y + 1, h, b(POLISHED_ANDESITE_SLAB));
            }
        }
    }
    // The Ayrton Light: a gilded lantern on a gilded gallery, glazed in tall slits.
    t.ring(72, 3, |_, _| Some(b(RAW_GOLD_BLOCK)));
    for y in 73..=75 {
        t.ring(y, 3, |_, u| {
            Some(b(if u % 2 == 0 {
                BLACK_STAINED_GLASS
            } else {
                RAW_GOLD_BLOCK
            }))
        });
    }
    t.fill(76, 3, b(POLISHED_ANDESITE));
    t.ring(76, 3, |_, u| {
        Some(if u.abs() == 3 {
            b(RAW_GOLD_BLOCK)
        } else {
            b(POLISHED_ANDESITE_SLAB)
        })
    });

    // Spire from 77 m, ribbed in gold.
    let halves = [3, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 0, 0, 0];
    for (i, &h) in halves.iter().enumerate() {
        let y = 77 + i as i32;
        let next = halves.get(i + 1).copied().unwrap_or(0);
        if h == 0 {
            t.set(C, y, C, b(ANDESITE_WALL));
            continue;
        }
        t.roof_layer(
            y,
            h,
            next,
            POLISHED_ANDESITE,
            POLISHED_ANDESITE_STAIRS,
            b(RAW_GOLD_BLOCK),
        );
        t.fill(y, h - 1, b(POLISHED_ANDESITE));
    }
    // Finial: orb and gilded spike.
    t.set(C, 91, C, b(ANDESITE_WALL));
    t.set(C, 92, C, b(GOLD_BLOCK));
    for y in 93..=95 {
        t.set(C, y, C, b(LIGHTNING_ROD));
    }
}

/// The bell frame behind the lancets, so they look into shadow, and the lantern's light.
fn interior(t: &mut Tower) {
    for y in 61..=64 {
        t.fill(y, 3, b(DARK_OAK_PLANKS));
    }
    for y in 73..=75 {
        t.set(C, y, C, b(GLOWSTONE));
    }
}

fn resolve(part: Part) -> BlockWithProperties {
    const INWARD: [StairFacing; 4] = [
        StairFacing::South,
        StairFacing::West,
        StairFacing::North,
        StairFacing::East,
    ];
    match part {
        Part::Block(block) => BlockWithProperties::simple(block),
        Part::Stair { block, face, top } => {
            let s = create_stair_with_properties(block, INWARD[face], StairShape::Straight);
            if top {
                top_stair(s)
            } else {
                s
            }
        }
        Part::SlabTop(block) => top_slab(block),
    }
}

fn finish(t: Tower) -> PalettizedSchematic {
    let mut palette: Vec<BlockWithProperties> = Vec::new();
    let mut slots: BTreeMap<Part, u8> = BTreeMap::new();
    let mut voxels = Vec::with_capacity(t.cells.len());
    for (&(x, y, z), &part) in &t.cells {
        debug_assert!((0..SIDE).contains(&x) && (0..SIDE).contains(&z));
        let slot = *slots.entry(part).or_insert_with(|| {
            palette.push(resolve(part));
            u8::try_from(palette.len() - 1).expect("the tower uses at most 256 block states")
        });
        voxels.push((x as i16, y as i16, z as i16, slot));
    }
    PalettizedSchematic {
        width: SIDE,
        length: SIDE,
        palette,
        voxels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn blocks(model: &PalettizedSchematic) -> HashMap<(i16, i16, i16), Block> {
        model
            .voxels
            .iter()
            .map(|&(x, y, z, slot)| ((x, y, z), model.palette[slot as usize].block))
            .collect()
    }

    #[test]
    fn every_face_is_the_same_turned_a_quarter() {
        let cells = blocks(&build());
        let c = C as i16;
        for (&(x, y, z), &block) in &cells {
            let turned = (c - (z - c), y, c + (x - c));
            assert_eq!(cells.get(&turned), Some(&block), "{x},{y},{z}");
        }
    }

    #[test]
    fn it_stands_96_metres_on_its_grid() {
        let model = build();
        let top = model.voxels.iter().map(|v| v.1).max();
        assert_eq!(top, Some(95));
        assert!(model.voxels.iter().all(|v| v.1 >= 0
            && (0..SIDE as i16).contains(&v.0)
            && (0..SIDE as i16).contains(&v.2)));
        let cells = blocks(&model);
        let south = C as i16 + 7;
        assert_eq!(
            cells.get(&(C as i16, DIAL_Y as i16, south)),
            Some(&LAPIS_BLOCK),
            "dial hub"
        );
        assert_eq!(
            cells.get(&(C as i16 + 1, DIAL_Y as i16, south)),
            Some(&WHITE_STAINED_GLASS)
        );
        assert_eq!(
            cells.get(&(C as i16 + 1, DIAL_Y as i16, south - 1)),
            Some(&SEA_LANTERN)
        );
    }
}
