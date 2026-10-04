//! Tall single rooms: worship, sports, gyms, auditoriums, warehouses, factories, barns.

use super::canvas::{facing_block, fence_run, grindstone, mix, pick, seat, top_slab, Canvas};
use super::uses::Faith;
use super::{chest, entry_u, FloorCtx};
use crate::block_definitions::*;
use crate::element_processing::subprocessor::buildings_loot::LootTheme;

pub(super) fn worship(c: &mut Canvas, zone: u16, faith: Faith, ctx: &FloorCtx) {
    let Some(f) = c.long_frame(zone) else {
        return;
    };
    let (w, d) = (f.width, f.depth);
    if w < 3 || d < 6 {
        return;
    }
    let mid = w / 2;
    let back = d - 1;
    let headroom = c.headroom();

    match faith {
        Faith::Muslim => {
            // Mihrab: a carved niche in the middle of the far wall, the minbar beside it.
            for dy in 2..=headroom.min(4) {
                let (x, z) = f.world(mid, back);
                c.mount(x, dy, z, f.front(), CHISELED_QUARTZ_BLOCK);
            }
            for (i, v) in (back - 3..back).enumerate() {
                let (x, z) = f.world(mid + 2, v);
                if i == 2 {
                    c.put(x, 1, z, QUARTZ_BLOCK);
                } else {
                    c.put_with(x, 1, z, seat(QUARTZ_STAIRS, f.front()));
                }
            }
            // Prayer rows of carpet over the whole hall.
            for (x, z) in c.cells(zone) {
                let v = f.local(x, z).1;
                if v >= 2 && c.is_free(x, z) {
                    let rug = if (v / 2) % 2 == 0 {
                        GREEN_CARPET
                    } else {
                        RED_CARPET
                    };
                    c.put(x, 1, z, rug);
                }
            }
        }
        Faith::Christian | Faith::Jewish | Faith::Other => {
            let altar_v = back - 2;
            if faith == Faith::Jewish {
                // The ark on the far wall, the reading desk in front of it.
                for du in -1..=1 {
                    let (x, z) = f.world(mid + du, back);
                    c.stack(x, z, &[DARK_OAK_PLANKS, DARK_OAK_PLANKS]);
                }
                let (x, z) = f.world(mid, back);
                c.put(x, 3, z, LANTERN);
                let (x, z) = f.world(mid, altar_v);
                c.put_with(x, 1, z, facing_block(LECTERN, f.back()));
            } else {
                // Altar, with a lectern to one side.
                for du in -1..=1 {
                    let (x, z) = f.world(mid + du, altar_v);
                    let block = if du == 0 {
                        CHISELED_QUARTZ_BLOCK
                    } else {
                        QUARTZ_BLOCK
                    };
                    if c.put(x, 1, z, block) && du != 0 {
                        c.put(x, 2, z, LANTERN);
                    }
                }
                if w >= 7 {
                    let (x, z) = f.world(mid + 3, altar_v - 1);
                    c.put_with(x, 1, z, facing_block(LECTERN, f.front()));
                }
            }
            match faith {
                Faith::Christian if headroom >= 5 => {
                    // A cross high on the far wall.
                    let (x, z) = f.world(mid, back);
                    for dy in 3..=5 {
                        c.mount(x, dy, z, f.front(), GOLD_BLOCK);
                    }
                    for du in [-1, 1] {
                        let (cx, cz) = f.world(mid + du, back);
                        c.mount(cx, 4, cz, f.front(), GOLD_BLOCK);
                    }
                }
                Faith::Other => {
                    // A statue behind the altar.
                    let (x, z) = f.world(mid, back);
                    c.stack(x, z, &[CHISELED_QUARTZ_BLOCK, GOLD_BLOCK]);
                }
                _ => {}
            }
            // Aisle carpet from the door to the altar, pews either side of it.
            let aisle = |u: i32| u == mid || (w >= 9 && u == mid - 1);
            for (x, z) in c.cells(zone) {
                let (u, v) = f.local(x, z);
                if !c.is_free(x, z) || v >= altar_v - 1 {
                    continue;
                }
                if aisle(u) {
                    c.put(x, 1, z, RED_CARPET);
                } else if matches!(faith, Faith::Christian | Faith::Jewish)
                    && v >= 2
                    && v < altar_v - 2
                    && v % 2 == 0
                    && u > 0
                    && u < w - 1
                {
                    c.put_with(x, 1, z, seat(ctx.wood.stairs, f.back()));
                } else if faith == Faith::Other && v >= 2 && v % 2 == 0 {
                    c.put(x, 1, z, RED_CARPET);
                }
            }
        }
    }
}

pub(super) fn sports_hall(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.long_frame(zone) else {
        return;
    };
    let (w, d) = (f.width, f.depth);
    let depth = c.depth(zone);
    // Wooden floor.
    for (x, z) in c.cells(zone) {
        c.floor(x, z, ctx.wood.planks);
    }
    // Stands along one long wall of a wide hall.
    let stands = w >= 16;
    if stands {
        for v in 2..d - 2 {
            let (x, z) = f.world(w - 1, v);
            if c.put(x, 1, z, ctx.wood.planks) {
                c.put_with(x, 2, z, seat(ctx.wood.stairs, f.dir(-1, 0)));
            }
            let (x, z) = f.world(w - 2, v);
            c.put_with(x, 1, z, seat(ctx.wood.stairs, f.dir(-1, 0)));
        }
    }
    // Court lines: the outline, the half-way line and the centre spot.
    let court_hi_u = if stands { w - 5 } else { w - 3 };
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if !c.is_free(x, z) || u < 2 || u > court_hi_u || v < 2 || v > d - 3 {
            continue;
        }
        let edge = u == 2 || u == court_hi_u || v == 2 || v == d - 3;
        let halfway = v == d / 2;
        if edge || halfway {
            c.put(x, 1, z, WHITE_CARPET);
        }
    }
    // Goals at both ends.
    let goal_u = (2 + court_hi_u) / 2;
    if c.headroom() >= 3 {
        for v in [3, d - 4] {
            for du in -1..=1 {
                let (x, z) = f.world(goal_u + du, v);
                if du != 0 {
                    c.put(x, 1, z, WHITE_CONCRETE);
                    c.put(x, 2, z, WHITE_CONCRETE);
                }
                c.put(x, 3, z, WHITE_CONCRETE);
            }
        }
    }
    // Team benches on the free side.
    for v in d / 2 - 3..d / 2 + 3 {
        if v == d / 2 {
            continue;
        }
        let (x, z) = f.world(0, v);
        if depth.get(x, z) >= 1 {
            c.put_with(x, 1, z, seat(ctx.wood.stairs, f.dir(1, 0)));
        }
    }
}

pub(super) fn gym(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    // A mirror wall at the back with weights below it.
    for ((x, z), n) in c.wall_cells(zone) {
        if n == f.front() {
            c.mount(x, 2, z, n, LIGHT_GRAY_STAINED_GLASS);
            if mix(x, z, ctx.seed).is_multiple_of(2) {
                c.put(x, 1, z, ANVIL);
            }
        }
    }
    let depth = c.depth(zone);
    let ue = entry_u(c, zone, &f);
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if depth.get(x, z) < 2 || v < 3 || (u - ue).rem_euclid(3) != 1 || v % 3 != 0 {
            continue;
        }
        match mix(x, z, ctx.seed ^ 0x6E) % 4 {
            0 => {
                c.put(x, 1, z, ANVIL);
            }
            1 => {
                c.put_with(x, 1, z, grindstone(f.front()));
            }
            2 => {
                c.put_with(x, 1, z, top_slab(POLISHED_BLACKSTONE_SLAB));
            }
            _ => {
                c.stack(x, z, &[IRON_BLOCK, IRON_BARS]);
            }
        }
    }
    // Mats over the rest of the front of the room.
    for (x, z) in c.cells(zone) {
        let v = f.local(x, z).1;
        if c.is_free(x, z) && (2..5).contains(&v) && depth.get(x, z) >= 2 {
            c.put(x, 1, z, LIGHT_GRAY_CARPET);
        }
    }
    let (x, z) = f.world(0, 1);
    c.put(x, 1, z, WATER_CAULDRON);
}

pub(super) fn auditorium(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.long_frame(zone) else {
        return;
    };
    let (w, d) = (f.width, f.depth);
    if d < 8 {
        return;
    }
    let headroom = c.headroom();
    // Stage across the far end with curtains at its sides, the screen above it.
    let stage_v = d - 4;
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if v >= stage_v && u > 0 && u < w - 1 && v < d - 1 {
            c.put(x, 1, z, ctx.wood.planks);
        }
    }
    for u in [1, w - 2] {
        let (x, z) = f.world(u, stage_v);
        for dy in 2..=headroom {
            c.put(x, dy, z, RED_WOOL);
        }
    }
    for u in 2..w - 2 {
        let (x, z) = f.world(u, d - 1);
        for dy in 3..=headroom.min(7) {
            c.mount(x, dy, z, f.front(), WHITE_WOOL);
        }
    }
    // Rows of seats facing the stage, aisles at the sides and down the middle.
    let mid = w / 2;
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        let aisle = u <= 1 || u >= w - 2 || (w >= 12 && u == mid);
        if !aisle && v >= 2 && v < stage_v - 2 && v % 2 == 0 {
            c.put_with(x, 1, z, seat(RED_NETHER_BRICK_STAIRS, f.back()));
        }
    }
}

pub(super) fn warehouse(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    let rack_height = (c.headroom() - 1).clamp(1, 4);
    let goods = [BARREL, BARREL, HAY_BALE, ctx.wood.planks, BARREL];
    let ue = entry_u(c, zone, &f);
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        // A loading area inside the door, racks in double rows behind it.
        let lane = (u - ue).rem_euclid(5);
        if v < 4 || depth.get(x, z) < 2 || !(lane == 2 || lane == 3) || v % 9 == 0 {
            continue;
        }
        let h = mix(x, z, ctx.seed);
        if h.is_multiple_of(29) && chest(c, x, z, LootTheme::Resources, ctx.salt) {
            continue;
        }
        let levels: Vec<Block> = (0..rack_height)
            .map(|i| pick(&goods, h >> (8 * i)))
            .collect();
        c.stack(x, z, &levels);
    }
    // Pallets along the walls.
    for ((x, z), _) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed ^ 0x9A);
        if f.local(x, z).1 >= 4 && h.is_multiple_of(3) {
            c.stack(x, z, &[BARREL, pick(&[HAY_BALE, BARREL], h >> 8)]);
        }
    }
}

pub(super) fn factory(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.long_frame(zone) else {
        return;
    };
    let (w, d) = (f.width, f.depth);
    let mid = w / 2;
    // A conveyor down the hall.
    let rail = if f.n.0 != 0 {
        RAIL_EAST_WEST
    } else {
        RAIL_NORTH_SOUTH
    };
    for v in 2..d - 2 {
        let (x, z) = f.world(mid, v);
        c.put(x, 1, z, rail);
    }
    // Machines either side of it.
    for v in (3..d - 3).step_by(4) {
        for side in [-2, 2] {
            let (x, z) = f.world(mid + side, v);
            let toward_belt = f.dir(-side.signum(), 0);
            match mix(x, z, ctx.seed) % 5 {
                0 => {
                    c.put_with(x, 1, z, facing_block(BLAST_FURNACE, toward_belt));
                }
                1 => {
                    c.put_with(x, 1, z, facing_block(FURNACE, toward_belt));
                }
                2 => {
                    c.stack(x, z, &[IRON_BLOCK, HOPPER]);
                }
                3 => {
                    c.put(x, 1, z, CAULDRON);
                }
                _ => {
                    c.put(x, 1, z, DISPENSER);
                }
            }
        }
    }
    // Workbenches along the walls.
    for ((x, z), n) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed ^ 0xFA);
        if f.local(x, z).1 < 2 || h.is_multiple_of(2) {
            continue;
        }
        match (h >> 8) % 6 {
            0 => c.put(x, 1, z, CRAFTING_TABLE),
            1 => c.put(x, 1, z, SMITHING_TABLE),
            2 => c.put_with(x, 1, z, grindstone(n)),
            3 => c.put(x, 1, z, ANVIL),
            4 => chest(c, x, z, LootTheme::Tools, ctx.salt),
            _ => c.put(x, 1, z, BARREL),
        };
    }
}

pub(super) fn barn(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.long_frame(zone) else {
        return;
    };
    let (w, d) = (f.width, f.depth);
    // A clear passage down the middle, stalls on both sides of it.
    let mid = w / 2;
    for v in 0..d {
        for u in [mid - 1, mid] {
            let (x, z) = f.world(u, v);
            c.keep(x, z);
        }
    }
    let along_x = f.t.0 != 0;
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if v % 4 == 0 && v > 0 && v < d - 1 && (u < mid - 2 || u > mid + 1) {
            c.put_with(x, 1, z, fence_run(ctx.wood.fence, along_x));
        }
    }
    // Feed and water at the wall end of each stall, hay stacked at the far end.
    for ((x, z), _) in c.wall_cells(zone) {
        let v = f.local(x, z).1;
        let h = mix(x, z, ctx.seed);
        if v == d - 1 {
            let height = (c.headroom() - 1).clamp(1, 3) as usize;
            c.stack(x, z, &[HAY_BALE, HAY_BALE, HAY_BALE][..height]);
        } else if v % 4 == 2 {
            c.put(x, 1, z, pick(&[HAY_BALE, WATER_CAULDRON, COMPOSTER], h));
        }
    }
    for ((x, z), _) in c.wall_cells(zone) {
        if mix(x, z, ctx.seed ^ 0xBA).is_multiple_of(17) {
            chest(c, x, z, LootTheme::Food, ctx.salt);
        }
    }
}
