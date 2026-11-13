//! Vineyards and orchards: trellis rows down the slope, and fruit trees in rows along the field.

use crate::block_definitions::*;
use crate::element_processing::area_layout::{cell_hash, mix, AreaMask, Frame};
use crate::element_processing::connected_blocks::connected_fence;
use crate::element_processing::landuse::ground_is_protected;
use crate::floodfill_cache::{in_hole, BuildingFootprintBitmap, Holes, RoadMaskBitmap};
use crate::ground_generation::value_noise_01;
use crate::land_cover::coord_hash;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

/// Blocks between vine rows, and the row length before a lane crosses them.
const VINE_PITCH: i32 = 3;
const VINE_RUN: i32 = 36;
/// A field steeper than this has its rows run down the slope, as hillside vineyards do.
const STEEP: f64 = 0.08;

pub fn vineyard_area(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    fill: &[(i32, i32)],
    road_mask: &RoadMaskBitmap,
    footprints: &BuildingFootprintBitmap,
    holes: Option<&Holes>,
) {
    let Some(frame) = Frame::of(&way.nodes) else {
        return;
    };
    let (dx, _) = row_axis(editor, way, &frame);
    let h = mix(way.id ^ 0x0071_11E5);
    let phase = (h % VINE_PITCH as u64) as i32;
    let lane_phase = ((h >> 8) % VINE_RUN as u64) as i32;
    // Some growers keep a green cover between the rows, others till it.
    let green = !(h >> 16).is_multiple_of(3);
    let canopy = if (h >> 20).is_multiple_of(4) {
        JUNGLE_LEAVES
    } else {
        OAK_LEAVES
    };
    let mask = AreaMask::new(fill, |x, z| !in_hole(holes, x, z));
    // A vine stands here, by position alone, so a wire knows its neighbours in any tile.
    let vine = |editor: &WorldEditor, x: i32, z: i32| {
        let (along, across) = if dx != 0 { (x, z) } else { (z, x) };
        mask.contains(x, z)
            && (across + phase).rem_euclid(VINE_PITCH) == 0
            && (along + lane_phase).rem_euclid(VINE_RUN) >= 2
            && !footprints.contains(x, z)
            && !near_road(road_mask, x, z)
            && !editor.surface_is_sealed(x, z)
            && !editor.is_lc_water(x, z)
    };

    for &(x, z) in fill {
        if in_hole(holes, x, z) || !editor.in_bounds(x, z) || ground_is_protected(editor, x, z) {
            continue;
        }
        let (along, across) = if dx != 0 { (x, z) } else { (z, x) };
        let on_row = (across + phase).rem_euclid(VINE_PITCH) == 0;
        let n = value_noise_01(x + 59, z - 13, 5);
        let ground = if on_row || !green && n > 0.25 || green && n > 0.85 {
            COARSE_DIRT
        } else {
            GRASS_BLOCK
        };
        editor.set_block(ground, x, 0, z, None, None);
        if !editor.owns(x, z) || editor.is_lc_water(x, z) {
            continue;
        }
        if !vine(editor, x, z) {
            if ground == GRASS_BLOCK && coord_hash(x, z ^ 0x51).is_multiple_of(5) {
                editor.set_block(GRASS, x, 1, z, None, None);
            }
            continue;
        }
        // Wire between the posts and the canopy trained along it; a bare post every few vines.
        let ground_y = editor.get_ground_level(x, z);
        let wired =
            |nx: i32, nz: i32| vine(editor, nx, nz) && editor.get_ground_level(nx, nz) == ground_y;
        let (fwd, back) = if dx != 0 {
            (wired(x + 1, z), wired(x - 1, z))
        } else {
            (wired(x, z + 1), wired(x, z - 1))
        };
        let wire = if dx != 0 {
            connected_fence(OAK_FENCE, false, false, fwd, back)
        } else {
            connected_fence(OAK_FENCE, back, fwd, false, false)
        };
        let y = editor.get_absolute_y(x, 1, z);
        if !editor.block_exists_absolute(x, y, z) {
            editor.set_block_with_properties_absolute(wire, x, y, z, None, None);
        }
        let post = (along + lane_phase).rem_euclid(8) == 4;
        let top = if post { OAK_FENCE } else { canopy };
        editor.set_block(top, x, 2, z, None, None);
    }
}

fn near_road(road_mask: &RoadMaskBitmap, x: i32, z: i32) -> bool {
    road_mask.contains(x, z)
        || [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .any(|(dx, dz)| road_mask.contains(x + dx, z + dz))
}

/// The world axis rows run along: down a steep field's slope, else along the field.
fn row_axis(editor: &WorldEditor, way: &ProcessedWay, frame: &Frame) -> (i32, i32) {
    // Least-squares plane through the outline's ground heights.
    let pts: Vec<(f64, f64, f64)> = way
        .nodes
        .iter()
        .map(|n| {
            (
                n.x as f64,
                n.z as f64,
                editor.get_ground_level(n.x, n.z) as f64,
            )
        })
        .collect();
    if pts.len() >= 4 {
        let k = pts.len() as f64;
        let (mx, mz, my) = pts.iter().fold((0.0, 0.0, 0.0), |a, p| {
            (a.0 + p.0 / k, a.1 + p.1 / k, a.2 + p.2 / k)
        });
        let (mut sxx, mut szz, mut sxz, mut sxy, mut szy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for &(x, z, y) in &pts {
            let (x, z, y) = (x - mx, z - mz, y - my);
            sxx += x * x;
            szz += z * z;
            sxz += x * z;
            sxy += x * y;
            szy += z * y;
        }
        let det = sxx * szz - sxz * sxz;
        if det.abs() > 1e-6 {
            let gx = (sxy * szz - szy * sxz) / det;
            let gz = (szy * sxx - sxy * sxz) / det;
            if gx.hypot(gz) > STEEP {
                return if gx.abs() >= gz.abs() { (1, 0) } else { (0, 1) };
            }
        }
    }
    frame.along_axis()
}

/// A fruit tree: trunk height, crown radius, and the leaves it bears.
#[derive(Clone, Copy)]
pub(crate) struct Fruit {
    pub trunk: i32,
    pub radius: f64,
    pub log: Block,
    pub leaves: Block,
    pub accent: Option<Block>,
}

/// Orchard spacing along and between rows, the share of places planted, and the tree.
struct Orchard {
    along: f64,
    across: f64,
    planted: u64,
    jitter: f64,
    fruit: Fruit,
    /// A bare strip along each row, as plantations keep under their trees.
    strip: bool,
}

impl Orchard {
    fn of(way: &ProcessedWay, scale: f64) -> Self {
        let tag = |k: &str| way.tags.get(k).map(String::as_str);
        let trees = tag("trees").unwrap_or("");
        let meadow = tag("orchard") == Some("meadow_orchard");
        let plantation = tag("orchard") == Some("plantation");
        let fruit = |trunk, radius, leaves, accent| Fruit {
            trunk,
            radius,
            log: OAK_LOG,
            leaves,
            accent,
        };
        let s = scale.clamp(0.25, 4.0);
        let o = |along: f64, across: f64, planted, jitter, fruit: Fruit, strip| Orchard {
            along: (along * s).max(2.0),
            across: (across * s).max(3.0),
            planted,
            jitter,
            fruit: Fruit {
                radius: (fruit.radius * s).max(1.0),
                trunk: ((fruit.trunk as f64 * s).round() as i32).max(1),
                ..fruit
            },
            strip,
        };
        if meadow {
            return o(
                11.0,
                11.0,
                80,
                2.0,
                fruit(3, 3.2, OAK_LEAVES, Some(FLOWERING_AZALEA_LEAVES)),
                false,
            );
        }
        match trees {
            "olive_trees" => o(7.0, 8.0, 95, 0.6, fruit(2, 2.6, AZALEA_LEAVES, None), false),
            "orange_trees" | "citrus_trees" | "lemon_trees" => {
                o(5.0, 6.0, 96, 0.0, fruit(1, 2.1, JUNGLE_LEAVES, None), false)
            }
            "oil_palms" | "coconut_palms" | "date_palms" | "palm_trees" => o(
                9.0,
                8.0,
                95,
                0.0,
                Fruit {
                    log: JUNGLE_LOG,
                    ..fruit(4, 3.2, JUNGLE_LEAVES, None)
                },
                false,
            ),
            "walnut_trees" | "almond_trees" | "chestnut_trees" | "hazel_plants" | "nut_trees" => {
                o(6.0, 7.0, 95, 0.3, fruit(2, 2.6, OAK_LEAVES, None), false)
            }
            "cherry_trees" => o(
                4.0,
                6.0,
                96,
                0.0,
                fruit(1, 1.8, OAK_LEAVES, Some(CHERRY_LEAVES)),
                true,
            ),
            // Apples, pears and plums trained as spindles, crowns touching along the row.
            "apple_trees" | "pear_trees" | "plum_trees" | "fruit_trees" => o(
                3.0,
                5.0,
                97,
                0.0,
                fruit(1, 1.5, OAK_LEAVES, Some(FLOWERING_AZALEA_LEAVES)),
                true,
            ),
            _ if plantation => o(
                3.0,
                5.0,
                97,
                0.0,
                fruit(1, 1.5, OAK_LEAVES, Some(FLOWERING_AZALEA_LEAVES)),
                true,
            ),
            // Unknown trees: a looser grid of round crowns.
            _ => o(
                5.0,
                6.0,
                92,
                0.4,
                fruit(2, 2.0, OAK_LEAVES, Some(FLOWERING_AZALEA_LEAVES)),
                false,
            ),
        }
    }
}

pub fn orchard_area(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    fill: &[(i32, i32)],
    road_mask: &RoadMaskBitmap,
    footprints: &BuildingFootprintBitmap,
    holes: Option<&Holes>,
) {
    let Some(frame) = Frame::of(&way.nodes) else {
        return;
    };
    let orchard = Orchard::of(way, editor.scale());
    let mask = AreaMask::new(fill, |x, z| !in_hole(holes, x, z));
    let reach = orchard.fruit.radius.ceil() + orchard.jitter.ceil() + 1.0;
    let ctx = Ctx {
        id: way.id,
        frame,
        orchard,
        mask,
    };

    for &(x, z) in fill {
        if in_hole(holes, x, z) || !editor.in_bounds(x, z) || ground_is_protected(editor, x, z) {
            continue;
        }
        let (a, c) = ctx.frame.local(x, z);
        let in_strip = ctx.orchard.strip && {
            let r = (c / ctx.orchard.across).floor();
            (c - (r + 0.5) * ctx.orchard.across).abs() < 0.75
        };
        let ground = if in_strip { COARSE_DIRT } else { GRASS_BLOCK };
        editor.set_block(ground, x, 0, z, None, None);
        // Crowns stay out of buildings, which the old forest trees were clipped against too.
        if !editor.owns(x, z) || editor.is_lc_water(x, z) || footprints.contains(x, z) {
            continue;
        }

        // Every tree whose crown can reach this column.
        let (s0, s1) = (
            ((a - reach) / ctx.orchard.along).floor() as i64,
            ((a + reach) / ctx.orchard.along).floor() as i64,
        );
        let (r0, r1) = (
            ((c - reach) / ctx.orchard.across).floor() as i64,
            ((c + reach) / ctx.orchard.across).floor() as i64,
        );
        let mut trunks: Vec<(i32, i32)> = (r0..=r1)
            .flat_map(|r| (s0..=s1).map(move |s| (s, r)))
            .filter_map(|(s, r)| ctx.tree(editor, road_mask, footprints, s, r))
            .collect();
        // This column's own trunk before any neighbour's crown.
        trunks.sort_by_key(|&t| t != (x, z));
        let mut any = false;
        for t in trunks {
            any |= ctx.orchard.fruit.grow(editor, t, x, z);
        }
        if !any && ground == GRASS_BLOCK && coord_hash(x ^ 0x0C, z).is_multiple_of(6) {
            editor.set_block(GRASS, x, 1, z, None, None);
        }
    }
}

struct Ctx {
    id: u64,
    frame: Frame,
    orchard: Orchard,
    mask: AreaMask,
}

impl Ctx {
    /// The trunk at lattice point (s, r), if a tree stands there.
    fn tree(
        &self,
        editor: &WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
        s: i64,
        r: i64,
    ) -> Option<(i32, i32)> {
        let h = cell_hash(self.id, s, r);
        if h % 100 >= self.orchard.planted {
            return None;
        }
        let j = self.orchard.jitter;
        let (ja, jc) = if j > 0.0 {
            (
                ((h >> 8) % 1000) as f64 / 1000.0 * 2.0 * j - j,
                ((h >> 20) % 1000) as f64 / 1000.0 * 2.0 * j - j,
            )
        } else {
            (0.0, 0.0)
        };
        let (x, z) = self.frame.column(
            (s as f64 + 0.5) * self.orchard.along + ja,
            (r as f64 + 0.5) * self.orchard.across + jc,
        );
        (self.mask.contains(x, z)
            && !road_mask.contains(x, z)
            && !footprints.contains(x, z)
            && !editor.surface_is_sealed(x, z)
            && !editor.is_lc_water(x, z))
        .then_some((x, z))
    }
}

impl Fruit {
    /// Calls `put` with the height above the trunk's base and the block of every part of this
    /// tree in the column (dx, dz) from its trunk; false when the tree does not reach it.
    pub fn parts(&self, dx: i32, dz: i32, x: i32, z: i32, mut put: impl FnMut(i32, Block)) -> bool {
        let horiz = ((dx * dx + dz * dz) as f64).sqrt();
        if horiz > self.radius + 0.5 {
            return false;
        }
        let mut any = false;
        if dx == 0 && dz == 0 {
            for y in 1..=self.trunk {
                put(y, self.log);
                any = true;
            }
        }
        // A crown a little flatter than a ball, sitting on the trunk.
        let cy = self.trunk as f64 + self.radius * 0.8;
        let top = (cy + self.radius * 0.8).ceil() as i32;
        for y in (self.trunk + 1).max(1)..=top {
            let dy = (y as f64 - cy) / 0.8;
            let rough = (coord_hash(x * 7 + y, z) % 100) as f64 / 250.0;
            if horiz * horiz + dy * dy <= (self.radius + 0.3 - rough).powi(2) {
                let accent = self
                    .accent
                    .filter(|_| coord_hash(x ^ y, z ^ 0x0A99) % 100 < 12);
                put(y, accent.unwrap_or(self.leaves));
                any = true;
            }
        }
        any
    }

    /// This tree's blocks in column (x, z), level with its trunk's base on sloping ground.
    pub fn grow(&self, editor: &mut WorldEditor, trunk: (i32, i32), x: i32, z: i32) -> bool {
        let base = editor.get_absolute_y(trunk.0, 0, trunk.1);
        self.parts(x - trunk.0, z - trunk.1, x, z, |y, block| {
            editor.set_block_absolute(block, x, base + y, z, None, None);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spindle_tree_has_a_trunk_under_a_crown() {
        let f = Fruit {
            trunk: 1,
            radius: 1.5,
            log: OAK_LOG,
            leaves: OAK_LEAVES,
            accent: None,
        };
        let column = |dx: i32| {
            let mut c = std::collections::BTreeMap::new();
            let reached = f.parts(dx, 0, 10 + dx, 10, |y, b| {
                c.entry(y).or_insert(b);
            });
            (reached, c)
        };
        let (_, c) = column(0);
        assert_eq!(c.get(&1), Some(&OAK_LOG));
        assert_eq!(c.get(&2), Some(&OAK_LEAVES));
        let (reached, side) = column(1);
        assert!(reached, "the crown reaches the next column");
        assert!(!side.contains_key(&1), "nothing at trunk height beside it");
        assert!(!column(3).0);
        // A palm at four times the scale still carries its crown.
        let palm = Fruit {
            trunk: 16,
            radius: 12.8,
            ..f
        };
        let mut top = 0;
        palm.parts(0, 0, 0, 0, |y, b| {
            if b == OAK_LEAVES {
                top = top.max(y);
            }
        });
        assert!(top > 30, "crown up to {top}");
    }
}

#[cfg(test)]
mod field_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::*;

    fn plant(tags: &[(&str, &str)]) -> WorldEditor<'static> {
        let xzbbox = Box::leak(Box::new(XZBBox::rect_from_xz_lengths(80.0, 50.0).unwrap()));
        let mut editor = test_editor(xzbbox);
        let way = rect_way(61, 2, 2, 77, 47, tags);
        let fill = crate::floodfill::flood_fill_area(
            &way.nodes.iter().map(|n| (n.x, n.z)).collect::<Vec<_>>(),
            None,
        );
        let (roads, built) = (
            RoadMaskBitmap::new_empty(),
            BuildingFootprintBitmap::new_empty(),
        );
        if tags.contains(&("landuse", "vineyard")) {
            vineyard_area(&mut editor, &way, &fill, &roads, &built, None);
        } else {
            orchard_area(&mut editor, &way, &fill, &roads, &built, None);
        }
        editor
    }

    #[test]
    fn vines_stand_in_rows_along_the_field() {
        let editor = plant(&[("landuse", "vineyard")]);
        // The field runs along x, so its rows do, three blocks apart.
        let mut rows = std::collections::BTreeSet::new();
        for z in 4..46 {
            if (10..70)
                .filter(|&x| editor.check_for_block(x, 1, z, Some(&[OAK_FENCE])))
                .count()
                > 40
            {
                rows.insert(z);
            }
        }
        assert!(rows.len() >= 12, "{rows:?}");
        let v: Vec<i32> = rows.into_iter().collect();
        assert!(v.windows(2).all(|w| w[1] - w[0] == VINE_PITCH), "{v:?}");
    }

    #[test]
    fn an_apple_plantation_grows_in_rows_of_small_trees() {
        let editor = plant(&[("landuse", "orchard"), ("trees", "apple_trees")]);
        let trunks = count_blocks(&editor, 3..77, 3..47, 1, &[OAK_LOG]);
        // A tree every 3 x 5 blocks.
        assert!(trunks > 150 && trunks < 260, "{trunks} trunks");
        assert_eq!(
            count_blocks(&editor, 3..77, 3..47, 5, &[OAK_LEAVES]),
            0,
            "kept low"
        );
    }
}
