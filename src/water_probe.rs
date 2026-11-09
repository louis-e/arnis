//! Reads the finished world along planned routes, just before each region is written out
//! and nothing will change it any more: for ships the top water block of a column and the
//! lowest block over it, such as a bridge deck; for walkers the ground they can stand on
//! with room for their head. So boats float on the water that was actually built and pass
//! under every bridge, and people walk on the real pavement rather than through a wall.

use fnv::FnvHashMap;
use std::sync::Mutex;

/// How far over and under the expected water level a column is searched.
const ABOVE: i32 = 110;
const BELOW: i32 = 40;
/// The levels round where a walker is expected that are looked at for ground: from
/// `FOOT_BELOW` under it to `FOOT_ABOVE` over it.
const FOOT_BELOW: i32 = 4;
const FOOT_ABOVE: i32 = 3;

/// What a block means to a hull or a walker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    /// Air, or something a body passes through: grass, a carpet, a rail, a sign.
    Open,
    Water,
    Solid,
    /// A bottom slab: half a block to stand on.
    Half,
    /// A fence, wall, pane or trunk: in the way, and nothing to step onto.
    Tall,
    /// Leaves, which a walker's head brushes through but nothing stands in.
    Leaves,
}

/// What a water column holds: its highest water block, under nothing but air and floating
/// plants, and the lowest block over that water a hull would hit; heights kept small.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    water: Option<i16>,
    overhead: Option<i16>,
}

impl Sample {
    fn new(water: Option<i32>, overhead: Option<i32>) -> Self {
        let small = |y: i32| y.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
        Self {
            water: water.map(small),
            overhead: overhead.map(small),
        }
    }

    pub fn water(&self) -> Option<i32> {
        self.water.map(i32::from)
    }

    /// Free blocks over the water block, up to the search height when nothing is there.
    pub fn clearance(&self) -> Option<i32> {
        let water = self.water()?;
        Some(self.overhead.map_or(ABOVE, |o| i32::from(o) - water - 1))
    }
}

/// The blocks a walker can stand on round a level, each with room over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footing {
    base: i16,
    levels: u8,
    /// The levels that are bottom slabs, stood on half a block lower.
    half: u8,
    /// Whether what stands at the level asked about is no more than a post or a trunk.
    thin: bool,
}

impl Footing {
    /// The height of the feet standing nearest `y`, the lower of two as near.
    pub fn nearest(&self, y: f64) -> Option<f64> {
        (0..8)
            .filter(|k| self.levels & (1 << k) != 0)
            .map(|k| {
                let top = if self.half & (1 << k) != 0 { 0.5 } else { 1.0 };
                f64::from(self.base) + f64::from(k) + top
            })
            .min_by(|a, b| (a - y).abs().total_cmp(&(b - y).abs()).then(a.total_cmp(b)))
    }

    pub fn thin(&self) -> bool {
        self.thin
    }

    /// A post or a trunk where the ground was planned: nothing to stand on within a block.
    pub fn post(&self) -> bool {
        self.thin && (FOOT_BELOW - 1..=FOOT_BELOW + 1).all(|k| self.levels & (1 << k) == 0)
    }
}

/// A column as (x, z, level).
type Column = (i32, i32, i32);

/// Columns read once their region is done.
pub struct Probe<S> {
    /// Each region's columns, sorted, so a sample is found by search and stored compactly.
    wanted: FnvHashMap<(i32, i32), Vec<Column>>,
    read: Mutex<FnvHashMap<(i32, i32), Vec<S>>>,
    /// Columns read round the samples `read_region_round` picked, by (x, z).
    round: Mutex<FnvHashMap<(i32, i32), S>>,
}

pub type WaterProbe = Probe<Sample>;
pub type WalkProbe = Probe<Footing>;

impl<S: Copy> Probe<S> {
    /// Columns as (x, z, level); a level of 0 for water lets the world say where to look.
    pub fn new(columns: impl IntoIterator<Item = Column>) -> Self {
        let mut wanted: FnvHashMap<(i32, i32), Vec<Column>> = FnvHashMap::default();
        for c in columns {
            wanted.entry((c.0 >> 9, c.1 >> 9)).or_default().push(c);
        }
        for columns in wanted.values_mut() {
            columns.sort_unstable();
            columns.dedup();
            columns.shrink_to_fit();
        }
        Self {
            wanted,
            read: Mutex::new(FnvHashMap::default()),
            round: Mutex::new(FnvHashMap::default()),
        }
    }

    pub fn regions(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.wanted.keys().copied()
    }

    /// Reads the columns of region (rx, rz) once, `column` turning (x, z, level) into a sample.
    pub fn read_region(&self, rx: i32, rz: i32, column: impl Fn(i32, i32, i32) -> S) {
        self.read_region_round(rx, rz, column, |_| false);
    }

    /// As `read_region`, also reading the columns two blocks round each sample `round`
    /// picks, at its level, while the region is still there to read.
    pub fn read_region_round(
        &self,
        rx: i32,
        rz: i32,
        column: impl Fn(i32, i32, i32) -> S,
        round: impl Fn(&S) -> bool,
    ) {
        let Some(columns) = self.wanted.get(&(rx, rz)) else {
            return;
        };
        if self.read.lock().is_ok_and(|r| r.contains_key(&(rx, rz))) {
            return;
        }
        let samples: Vec<S> = columns.iter().map(|&c| column(c.0, c.1, c.2)).collect();
        let mut extra = Vec::new();
        for (&(x, z, level), s) in columns.iter().zip(&samples) {
            if !round(s) {
                continue;
            }
            for (dx, dz) in (-2..=2).flat_map(|dx| (-2..=2).map(move |dz| (dx, dz))) {
                let (x, z) = (x + dx, z + dz);
                if (dx, dz) != (0, 0) && (x >> 9, z >> 9) == (rx, rz) {
                    extra.push(((x, z), column(x, z, level)));
                }
            }
        }
        if let Ok(mut round) = self.round.lock() {
            round.extend(extra);
        }
        if let Ok(mut read) = self.read.lock() {
            read.insert((rx, rz), samples);
        }
    }

    pub fn get(&self, x: i32, z: i32, level: i32) -> Option<S> {
        let region = (x >> 9, z >> 9);
        let asked = || {
            let i = self
                .wanted
                .get(&region)?
                .binary_search(&(x, z, level))
                .ok()?;
            self.read.lock().ok()?.get(&region)?.get(i).copied()
        };
        asked().or_else(|| self.round.lock().ok()?.get(&(x, z)).copied())
    }
}

/// The heights a water column round `expect` is searched over.
pub fn water_span(expect: i32) -> (i32, i32) {
    (expect - BELOW, expect + ABOVE)
}

/// The water and what spans it in a column, searched round `expect` down from `top`, the
/// highest block there that is not air.
pub fn water_column(block: impl Fn(i32) -> Spot, expect: i32, top: i32) -> Sample {
    let mut overhead = None;
    for y in (expect - BELOW..=top.min(expect + ABOVE)).rev() {
        match block(y) {
            Spot::Open => {}
            Spot::Water => return Sample::new(Some(y), overhead),
            Spot::Solid | Spot::Half | Spot::Tall | Spot::Leaves => overhead = Some(y),
        }
    }
    Sample::new(None, overhead)
}

/// Where a walker can stand round `expect`: a solid block with an open one over it and
/// room for a head, under a low branch at most.
pub fn footing(block: impl Fn(i32) -> Spot, expect: i32) -> Footing {
    let base = expect - FOOT_BELOW;
    let spots: Vec<Spot> = (base..=expect + FOOT_ABOVE + 2).map(&block).collect();
    let (mut levels, mut half) = (0u8, 0u8);
    for k in 0..=(FOOT_BELOW + FOOT_ABOVE) as usize {
        if matches!(spots[k], Spot::Solid | Spot::Half)
            && spots[k + 1] == Spot::Open
            && matches!(spots[k + 2], Spot::Open | Spot::Leaves)
        {
            levels |= 1 << k;
            if spots[k] == Spot::Half {
                half |= 1 << k;
            }
        }
    }
    Footing {
        base: base.clamp(i16::MIN.into(), i16::MAX.into()) as i16,
        levels,
        half,
        // A lamp, a bollard or a trunk is no full block from head height up; a wall is.
        thin: spots[FOOT_BELOW as usize + 2..=FOOT_BELOW as usize + 4]
            .iter()
            .all(|&s| s != Spot::Solid),
    }
}

/// Whether a walker passes through a block of this name, as through grass or a carpet.
fn walk_through(name: &str) -> bool {
    const OPEN: [&str; 39] = [
        "air",
        "cave_air",
        "void_air",
        "light",
        "snow",
        "short_grass",
        "tall_grass",
        "short_dry_grass",
        "tall_dry_grass",
        "fern",
        "large_fern",
        "dead_bush",
        "bush",
        "firefly_bush",
        "vine",
        "sunflower",
        "lilac",
        "rose_bush",
        "peony",
        "dandelion",
        "poppy",
        "blue_orchid",
        "allium",
        "azure_bluet",
        "oxeye_daisy",
        "cornflower",
        "lily_of_the_valley",
        "torchflower",
        "pink_petals",
        "wildflowers",
        "leaf_litter",
        "rail",
        "powered_rail",
        "detector_rail",
        "activator_rail",
        "redstone_wire",
        "tripwire",
        "lever",
        "flower_pot",
    ];
    const ENDS: [&str; 8] = [
        "_carpet",
        "_tulip",
        "_sapling",
        "_pressure_plate",
        "_button",
        "_sign",
        "_banner",
        "_torch",
    ];
    OPEN.contains(&name) || ENDS.iter().any(|e| name.ends_with(e))
}

/// What a walker makes of a named block that is not water; `bottom` is whether a slab is
/// the lower half of its block.
pub fn walk_spot(name: &str, bottom: impl FnOnce() -> bool) -> Spot {
    if walk_through(name) {
        Spot::Open
    } else if name.ends_with("_slab") && bottom() {
        Spot::Half
    } else if name.ends_with("_leaves") {
        Spot::Leaves
    } else if too_tall(name) {
        Spot::Tall
    } else {
        Spot::Solid
    }
}

/// Whether a block of this name is a post to pass rather than a step, as a fence, a wall,
/// a tree trunk or a bin is.
fn too_tall(name: &str) -> bool {
    const ENDS: [&str; 14] = [
        "cauldron",
        "barrel",
        "redstone_block",
        "composter",
        "_fence",
        "_fence_gate",
        "_wall",
        "_pane",
        "_bars",
        "glass_pane",
        "_log",
        "_wood",
        "_stem",
        "_hyphae",
    ];
    ENDS.iter().any(|e| name.ends_with(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bridge_over_the_water_is_its_overhead_and_land_has_no_water() {
        let probe = WaterProbe::new([(5, 5, 0), (600, 5, 0)]);
        let world = |x: i32, y: i32| match (x, y) {
            (5, 70) => Spot::Solid,
            (5, ..=60) => Spot::Water,
            (600, ..=62) => Spot::Solid,
            _ => Spot::Open,
        };
        probe.read_region(0, 0, |x, _, _| water_column(|y| world(x, y), 60, 80));
        probe.read_region(1, 0, |x, _, _| water_column(|y| world(x, y), 60, 80));
        let water = probe.get(5, 5, 0).unwrap();
        assert_eq!(water.water(), Some(60));
        assert_eq!(water.clearance(), Some(9));
        assert_eq!(probe.get(600, 5, 0).unwrap().water(), None);
        assert!(probe.get(7, 7, 0).is_none());
    }

    #[test]
    fn a_walker_stands_on_the_ground_and_not_in_the_leaves_over_it() {
        // Ground at 63, a canopy at 66 with air over it.
        let world = |y: i32| match y {
            ..=63 => Spot::Solid,
            66 => Spot::Solid,
            _ => Spot::Open,
        };
        let f = footing(world, 64);
        assert_eq!(f.nearest(64.0), Some(64.0));
        // Under a wall there is nowhere to stand, and a wall is no post.
        let wall = footing(|y| if y <= 70 { Spot::Solid } else { Spot::Open }, 64);
        assert_eq!(wall.nearest(64.0), None);
        assert!(!wall.thin());
        // Nor is a wall with a window at head height.
        let window = footing(
            |y| match y {
                ..=64 | 67.. => Spot::Solid,
                _ => Spot::Tall,
            },
            63,
        );
        assert!(!window.thin());
        // A slab is stood on half a block up.
        let slab = footing(
            |y| match y {
                ..=63 => Spot::Solid,
                64 => Spot::Half,
                _ => Spot::Open,
            },
            63,
        );
        assert_eq!(slab.nearest(64.0), Some(64.5));
        // Nor on a fence, which is no step up.
        let fence = footing(
            |y| match y {
                ..=63 => Spot::Solid,
                64 => Spot::Tall,
                _ => Spot::Open,
            },
            63,
        );
        assert_eq!(fence.nearest(65.0), None);
        assert!(fence.thin());
        // A low branch at head height is ducked under, a hedge is not walked through.
        let leaves = |at: i32| {
            move |y: i32| match y {
                ..=63 => Spot::Solid,
                y if y == at => Spot::Leaves,
                _ => Spot::Open,
            }
        };
        assert_eq!(footing(leaves(65), 63).nearest(64.0), Some(64.0));
        assert_eq!(footing(leaves(64), 63).nearest(64.0), None);
    }

    #[test]
    fn grass_and_carpets_are_walked_through_but_blocks_are_not() {
        for open in [
            "air",
            "short_grass",
            "white_carpet",
            "snow",
            "poppy",
            "rail",
            "oak_sign",
        ] {
            assert!(walk_through(open), "{open}");
        }
        for solid in [
            "stone_bricks",
            "snow_block",
            "grass_block",
            "oak_fence",
            "bricks",
            "light_gray_concrete",
        ] {
            assert!(!walk_through(solid), "{solid}");
        }
        for tall in [
            "oak_fence",
            "cobblestone_wall",
            "iron_bars",
            "glass_pane",
            "spruce_log",
            "cauldron",
            "barrel",
            "redstone_block",
        ] {
            assert!(too_tall(tall), "{tall}");
        }
        for low in [
            "stone_bricks",
            "oak_wall_sign",
            "wall_torch",
            "andesite_slab",
        ] {
            assert!(!too_tall(low), "{low}");
        }
        assert_eq!(walk_spot("oak_slab", || true), Spot::Half);
        assert_eq!(walk_spot("oak_slab", || false), Spot::Solid);
    }
}
