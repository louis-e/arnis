//! Schools, kindergartens, libraries, practices, hospitals, hotels, museums and stations.

use super::canvas::{bed, mix, pick, seat, top_slab, Canvas, Frame};
use super::commerce::eatery;
use super::uses::Eatery;
use super::{book_shelf, chest, entry_u, plant, table_set, FloorCtx};
use crate::block_definitions::*;
use crate::element_processing::subprocessor::buildings_loot::LootTheme;

/// Frame facing along the corridor, so the class looks at a side wall.
fn sideways_frame(c: &Canvas, zone: u16) -> Option<Frame> {
    let b = c.bounds(zone)?;
    match c.entry(zone) {
        Some(e) => Some(Frame::new(b, (-e.inward.1, e.inward.0))),
        None => c.long_frame(zone),
    }
}

pub(super) fn school(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    match c.split_corridor(zone, 8, 6) {
        Some(split) => {
            corridor_benches(c, split.hall, ctx);
            for room in split.rooms {
                classroom(c, room, ctx);
            }
        }
        None => classroom(c, zone, ctx),
    }
}

/// Benches here and there along one side of a corridor.
fn corridor_benches(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    c.focus(zone);
    let Some(b) = c.bounds(zone) else {
        return;
    };
    let side = if b.long_x() { (0, 1) } else { (1, 0) };
    for ((x, z), n) in c.wall_cells(zone) {
        if n == side && mix(x, z, ctx.seed).is_multiple_of(5) {
            c.put_with(x, 1, z, seat(ctx.wood.stairs, n));
        }
    }
}

/// Board on the front wall, teacher's desk, rows of desks facing it, shelves at the back.
fn classroom(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    c.focus(zone);
    let Some(f) = sideways_frame(c, zone) else {
        return;
    };
    let mid = f.width / 2;
    // Board.
    for u in 1..f.width - 1 {
        let (x, z) = f.world(u, 0);
        c.mount(x, 2, z, f.back(), GREEN_CONCRETE);
        if c.headroom() >= 4 {
            c.mount(x, 3, z, f.back(), GREEN_CONCRETE);
        }
    }
    // Teacher.
    let (dx, dz) = f.world(mid, 2);
    c.put_with(dx, 1, dz, top_slab(ctx.wood.slab));
    let (sx, sz) = f.world(mid, 1);
    c.put_with(sx, 1, sz, seat(ctx.wood.stairs, f.back()));
    let (lx, lz) = f.world(mid + 2, 2);
    c.put_with(lx, 1, lz, super::canvas::facing_block(LECTERN, f.back()));
    // Pupils: desk rows with chairs behind them, an aisle every third column.
    let mut v = 4;
    while v + 1 < f.depth - 1 {
        for u in 1..f.width - 1 {
            if u % 3 == 0 {
                continue;
            }
            let (x, z) = f.world(u, v);
            let (cx, cz) = f.world(u, v + 1);
            if c.is_free(x, z) && c.is_free(cx, cz) {
                c.put_with(x, 1, z, top_slab(ctx.wood.slab));
                c.put_with(cx, 1, cz, seat(ctx.wood.stairs, f.front()));
            }
        }
        v += 2;
    }
    // Shelves and a plant along the back wall.
    for ((x, z), n) in c.wall_cells(zone) {
        if f.local(x, z).1 == f.depth - 1 && mix(x, z, ctx.seed).is_multiple_of(2) {
            c.put_with(x, 1, z, book_shelf(n, mix(x, z, ctx.seed)));
        }
    }
    plant(c, &f, f.width - 1, f.depth - 1, ctx.seed);
}

pub(super) fn kindergarten(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    // Low tables with chairs all round.
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if depth.get(x, z) >= 3 && u % 5 == 2 && v % 5 == 3 {
            table_set(c, &f, u, v, ctx.wood, &[(-1, 0), (1, 0), (0, -1), (0, 1)]);
        }
    }
    // Toy chests, soft blocks and a nap corner along the walls.
    for ((x, z), n) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed);
        let v = f.local(x, z).1;
        if v > f.depth / 2
            && h.is_multiple_of(3)
            && bed(c, (x + n.0, z + n.1), (-n.0, -n.1), WHITE_BED)
        {
            continue;
        }
        match h % 5 {
            0 => {
                chest(c, x, z, LootTheme::Mixed, ctx.salt);
            }
            1 => {
                c.put(x, 1, z, NOTE_BLOCK);
            }
            2 => {
                c.put(
                    x,
                    1,
                    z,
                    pick(&[RED_WOOL, YELLOW_WOOL, BLUE_WOOL, GREEN_WOOL], h >> 8),
                );
            }
            _ => {}
        }
    }
    // Bright carpet over the rest of the floor.
    for (x, z) in c.cells(zone) {
        if c.is_free(x, z) {
            let (u, v) = f.local(x, z);
            let colour = pick(
                &[RED_CARPET, LIGHT_BLUE_CARPET, GREEN_CARPET, WHITE_CARPET],
                ((u / 2 + v / 2) % 4) as u64,
            );
            c.put(x, 1, z, colour);
        }
    }
}

pub(super) fn library(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    let tall = c.headroom() >= 3;
    let ue = entry_u(c, zone, &f);
    // Desk by the door.
    let side = if ue + 3 < f.width { 1 } else { -1 };
    for v in 1..=2 {
        let (x, z) = f.world(ue + 2 * side, v);
        c.put(x, 1, z, ctx.wood.planks);
    }
    // Bookshelves round the walls.
    for ((x, z), n) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed);
        if c.put_with(x, 1, z, book_shelf(n, h)) && tall && !c.window_behind(x, z, n) {
            c.put(x, 2, z, BOOKSHELF);
        }
    }
    // Reading tables in front, stacks behind with a gap down the middle.
    let stacks_from = (f.depth / 3).max(5);
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        if depth.get(x, z) < 3 || !c.is_free(x, z) {
            continue;
        }
        let (u, v) = f.local(x, z);
        if v < stacks_from {
            if v >= 3
                && (u - ue).rem_euclid(4) == 2
                && v % 3 == 0
                && table_set(c, &f, u, v, ctx.wood, &[(-1, 0), (1, 0)])
            {
                c.put(x, 2, z, LANTERN);
            }
        } else if (v - stacks_from) % 3 == 0 && (u - ue).abs() > 1 && u % 7 != 0 {
            c.put(x, 1, z, BOOKSHELF);
            if tall {
                c.put(x, 2, z, BOOKSHELF);
            }
        }
    }
    plant(c, &f, 0, 1, ctx.seed);
}

pub(super) fn clinic(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    match c.split_corridor(zone, 5, 4) {
        Some(split) => {
            reception(c, split.hall, ctx);
            for room in split.rooms {
                exam_room(c, room, ctx);
            }
        }
        None => {
            let back = c
                .frame(zone)
                .and_then(|f| c.split_back(zone, &f, f.depth - 5));
            reception(c, zone, ctx);
            if let Some(back) = back {
                exam_room(c, back, ctx);
            }
        }
    }
}

/// Front desk by the door and chairs along the walls.
fn reception(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    c.focus(zone);
    let Some(f) = c.frame(zone) else {
        return;
    };
    let ue = entry_u(c, zone, &f);
    for v in 1..=2 {
        let (x, z) = f.world(ue + 2, v);
        c.put(x, 1, z, QUARTZ_BLOCK);
    }
    for ((x, z), n) in c.wall_cells(zone) {
        let (u, v) = f.local(x, z);
        if v >= 1 && (u - ue).abs() > 3 && !mix(x, z, ctx.seed).is_multiple_of(3) {
            c.put_with(x, 1, z, seat(ctx.wood.stairs, n));
        }
    }
    plant(c, &f, 0, 1, ctx.seed);
}

/// Couch against the far wall, a desk, a basin and a cabinet.
fn exam_room(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    c.focus(zone);
    let Some(f) = c.frame(zone) else {
        return;
    };
    let mut walls = c.wall_cells(zone);
    walls.sort_by_key(|&((x, z), _)| std::cmp::Reverse(f.local(x, z).1));
    for &((x, z), n) in &walls {
        if bed(c, (x + n.0, z + n.1), (-n.0, -n.1), WHITE_BED) {
            break;
        }
    }
    let mut k = 0;
    for &((x, z), n) in walls.iter().rev() {
        let placed = match k {
            0 => c.put(x, 1, z, WATER_CAULDRON),
            1 => c.put_with(x, 1, z, top_slab(SMOOTH_QUARTZ_SLAB)),
            2 => c.put(x, 1, z, BARREL),
            _ => break,
        };
        if placed {
            if k == 1 {
                let (sx, sz) = (x + n.0, z + n.1);
                c.put_with(sx, 1, sz, seat(ctx.wood.stairs, (-n.0, -n.1)));
            }
            k += 1;
        }
    }
}

pub(super) fn hospital(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    if ctx.floor == 0 && ctx.floors > 1 {
        return clinic(c, zone, ctx);
    }
    match c.split_corridor(zone, 7, 5) {
        Some(split) => {
            corridor_benches(c, split.hall, ctx);
            for room in split.rooms {
                ward(c, room);
            }
        }
        None => ward(c, zone),
    }
}

/// Beds with their heads to the side walls, a cabinet beside each, a basin by the door.
fn ward(c: &mut Canvas, zone: u16) {
    c.focus(zone);
    let Some(f) = c.frame(zone) else {
        return;
    };
    for ((x, z), n) in c.wall_cells(zone) {
        let (_, v) = f.local(x, z);
        let side_wall = n != f.back() && n != f.front();
        if !side_wall || v < 2 {
            continue;
        }
        if v % 3 == 2 {
            bed(c, (x + n.0, z + n.1), (-n.0, -n.1), WHITE_BED);
        } else if v % 3 == 0 {
            c.put(x, 1, z, BARREL);
            c.put(x, 2, z, EMPTY_FLOWER_POT);
        }
    }
    let (x, z) = f.world(0, 1);
    c.put(x, 1, z, WATER_CAULDRON);
}

pub(super) fn hotel(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    if ctx.floor == 0 && ctx.floors > 1 {
        return lobby(c, zone, ctx);
    }
    match c.split_corridor(zone, 5, 5) {
        Some(split) => {
            for (x, z) in c.cells(split.hall) {
                if c.is_free(x, z) {
                    c.put(x, 1, z, RED_CARPET);
                }
            }
            for room in split.rooms {
                hotel_room(c, room, ctx);
            }
        }
        None => hotel_room(c, zone, ctx),
    }
}

/// Reception, sofas round low tables, and a restaurant at the back of a big hotel.
fn lobby(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    c.focus(zone);
    let Some(f) = c.frame(zone) else {
        return;
    };
    if f.depth >= 16 && c.area(zone) >= 200 {
        if let Some(back) = c.split_back(zone, &f, f.depth / 2) {
            eatery(c, back, Eatery::Restaurant, ctx);
            c.focus(zone);
        }
    }
    let ue = entry_u(c, zone, &f);
    for v in 2..=4 {
        let (x, z) = f.world(ue + 3, v);
        c.put(x, 1, z, ctx.wood.planks);
    }
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if depth.get(x, z) >= 3 && (u - ue).rem_euclid(6) == 3 && v % 5 == 3 && v > 3 {
            // A low table with a sofa either side.
            if c.put_with(x, 1, z, top_slab(ctx.wood.slab)) {
                for du in [-1, 1] {
                    let (sx, sz) = f.world(u + du, v);
                    c.put_with(sx, 1, sz, seat(ctx.wood.stairs, f.dir(-du, 0)));
                }
            }
        }
    }
    for ((x, z), _) in c.wall_cells(zone) {
        if mix(x, z, ctx.seed).is_multiple_of(7) {
            c.put(x, 1, z, pick(&[AZALEA, FLOWERING_AZALEA], mix(x, z, 3)));
        }
    }
    for (x, z) in c.cells(zone) {
        if c.is_free(x, z) && depth.get(x, z) >= 2 {
            c.put(x, 1, z, RED_CARPET);
        }
    }
}

/// A bed against the far wall with bedside tables, a wardrobe and a desk.
fn hotel_room(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    c.focus(zone);
    let Some(f) = c.frame(zone) else {
        return;
    };
    let mid = f.width / 2;
    let back = f.depth - 1;
    let mut slept = false;
    for du in [0, -1, 1, -2, 2] {
        if bed(c, f.world(mid + du, back - 1), f.back(), WHITE_BED) {
            slept = true;
            for side in [-1, 1] {
                let (nx, nz) = f.world(mid + du + side, back);
                if c.put(nx, 1, nz, ctx.wood.planks) {
                    c.put(nx, 2, nz, LANTERN);
                }
            }
            break;
        }
    }
    let mut desk = false;
    for ((x, z), n) in c.wall_cells(zone) {
        let v = f.local(x, z).1;
        if v == 0 {
            continue;
        }
        if !desk && v >= 1 && v < back - 1 && n != f.front() {
            if c.put_with(x, 1, z, top_slab(ctx.wood.slab)) {
                c.put_with(x + n.0, 1, z + n.1, seat(ctx.wood.stairs, (-n.0, -n.1)));
                desk = true;
            }
        } else if v == 1 {
            c.put_with(x, 1, z, book_shelf(n, mix(x, z, ctx.seed)));
        }
    }
    if slept {
        for (x, z) in c.cells(zone) {
            if c.is_free(x, z) {
                c.put(x, 1, z, LIGHT_GRAY_CARPET);
            }
        }
    }
}

pub(super) fn museum(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    let ue = entry_u(c, zone, &f);
    let exhibits = [
        AMETHYST_CLUSTER,
        GOLD_BLOCK,
        LANTERN,
        BREWING_STAND,
        ANVIL,
        POTTED_BLUE_ORCHID,
        END_ROD,
        CHISELED_STONE_BRICKS,
    ];
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if depth.get(x, z) >= 3 && (u - ue).rem_euclid(4) == 2 && v % 4 == 3 {
            let h = mix(x, z, ctx.seed);
            if c.put(
                x,
                1,
                z,
                pick(&[CHISELED_QUARTZ_BLOCK, POLISHED_ANDESITE], h),
            ) {
                c.put(x, 2, z, pick(&exhibits, h >> 8));
            }
        }
    }
    // Cases along the walls, a ticket desk by the door.
    for ((x, z), _) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed ^ 0x3E);
        if f.local(x, z).1 >= 2 && h.is_multiple_of(3) && c.put(x, 1, z, SMOOTH_QUARTZ) {
            c.put(x, 2, z, pick(&exhibits, h >> 8));
        }
    }
    for v in 1..=2 {
        let (x, z) = f.world(ue + 2, v);
        c.put(x, 1, z, ctx.wood.planks);
    }
}

pub(super) fn station(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.long_frame(zone) else {
        return;
    };
    // Ticket windows along one side wall near the entrance.
    for ((x, z), n) in c.wall_cells(zone) {
        let v = f.local(x, z).1;
        if n == f.dir(-1, 0) && (2..=6).contains(&v) {
            c.put(x, 1, z, POLISHED_ANDESITE);
            if c.headroom() >= 3 {
                c.put(x, 2, z, IRON_BARS);
            }
        }
    }
    // Departure board high on the far wall.
    let board_y = c.headroom().min(4);
    for du in -2..=2 {
        let (x, z) = f.world(f.width / 2 + du, f.depth - 1);
        c.mount(x, board_y, z, f.front(), BLACK_CONCRETE);
    }
    // Back to back benches across the hall.
    let depth = c.depth(zone);
    let ue = entry_u(c, zone, &f);
    for (x, z) in c.cells(zone) {
        if depth.get(x, z) < 3 {
            continue;
        }
        let (u, v) = f.local(x, z);
        if !(1..=3).contains(&(u - ue).rem_euclid(5)) || v < 4 {
            continue;
        }
        match v % 5 {
            1 => {
                c.put_with(x, 1, z, seat(ctx.wood.stairs, f.front()));
            }
            2 => {
                c.put_with(x, 1, z, seat(ctx.wood.stairs, f.back()));
            }
            _ => {}
        }
    }
    // A kiosk in a corner by the door.
    let (kx, kz) = f.world(f.width - 2, 1);
    if c.put(kx, 1, kz, ctx.wood.planks) {
        c.put(kx, 2, kz, CAKE);
    }
    plant(c, &f, 0, 1, ctx.seed);
}
