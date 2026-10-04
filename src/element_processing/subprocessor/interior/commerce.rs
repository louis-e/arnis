//! Shops, supermarkets, places to eat, offices, banks and workshops.

use super::canvas::{
    facing_block, grindstone, hanging_lantern, mix, pick, seat, top_slab, Canvas, Frame,
};
use super::uses::{Eatery, Goods};
use super::{book_shelf, chest, entry_u, plant, table_set, FloorCtx};
use crate::block_definitions::*;
use crate::element_processing::subprocessor::buildings_loot::LootTheme;

/// Full blocks that read as stocked shelving.
fn stock(goods: Goods) -> &'static [Block] {
    match goods {
        Goods::General => &[BARREL, BARREL, HAY_BALE, PUMPKIN, COMPOSTER],
        Goods::Bakery => &[HAY_BALE, BARREL, HAY_BALE, BROWN_TERRACOTTA],
        Goods::Butcher => &[SMOKER, BARREL, WHITE_CONCRETE],
        Goods::Grocery => &[HAY_BALE, MELON, PUMPKIN, BARREL, COMPOSTER],
        Goods::Books => &[BOOKSHELF, BOOKSHELF, CHISELLED_BOOKSHELF],
        Goods::Clothes => &[
            WHITE_WOOL,
            RED_WOOL,
            BLUE_WOOL,
            YELLOW_WOOL,
            GREEN_WOOL,
            BLACK_WOOL,
            CYAN_WOOL,
            GRAY_WOOL,
        ],
        Goods::Electronics => &[
            BLACK_CONCRETE,
            GRAY_CONCRETE,
            REDSTONE_LAMP,
            LIGHT_GRAY_CONCRETE,
        ],
        Goods::Hardware => &[BARREL, IRON_BLOCK, BARREL, CHEST, SMITHING_TABLE],
        Goods::Pharmacy => &[QUARTZ_BLOCK, WHITE_CONCRETE, CHISELLED_BOOKSHELF],
        Goods::Florist => &[
            MOSS_BLOCK,
            AZALEA_LEAVES,
            FLOWERING_AZALEA_LEAVES,
            OAK_LEAVES,
        ],
        Goods::Jewelry => &[SMOOTH_QUARTZ, CHISELED_QUARTZ_BLOCK],
        Goods::Toys => &[
            RED_CONCRETE,
            YELLOW_CONCRETE,
            BLUE_CONCRETE,
            LIME_CONCRETE,
            MAGENTA_CONCRETE,
            ORANGE_CONCRETE,
        ],
        Goods::Furniture => &[BOOKSHELF, BARREL, CHISELLED_BOOKSHELF],
        Goods::Drinks => &[BARREL, BARREL, BARREL, HAY_BALE],
        Goods::Salon => &[WHITE_CONCRETE, QUARTZ_BLOCK],
    }
}

/// Small things set out on counters and display tables.
fn wares(goods: Goods) -> &'static [Block] {
    match goods {
        Goods::Bakery => &[CAKE, CAKE, EMPTY_FLOWER_POT],
        Goods::Pharmacy => &[BREWING_STAND, EMPTY_FLOWER_POT],
        Goods::Florist => &[
            POTTED_RED_TULIP,
            POTTED_DANDELION,
            POTTED_BLUE_ORCHID,
            FLOWER_POT,
        ],
        Goods::Jewelry => &[AMETHYST_CLUSTER, LANTERN, AMETHYST_CLUSTER],
        Goods::Electronics => &[DAYLIGHT_DETECTOR, LANTERN],
        Goods::Drinks => &[BREWING_STAND, EMPTY_FLOWER_POT],
        Goods::Books => &[LECTERN],
        Goods::Toys => &[NOTE_BLOCK, EMPTY_FLOWER_POT],
        _ => &[],
    }
}

fn counter_block(goods: Goods, ctx: &FloorCtx) -> Block {
    match goods {
        Goods::Pharmacy | Goods::Jewelry | Goods::Salon => QUARTZ_BLOCK,
        Goods::Butcher => WHITE_CONCRETE,
        Goods::Electronics => POLISHED_ANDESITE,
        _ => ctx.wood.planks,
    }
}

/// A short counter beside the entry with room behind it for staff.
fn cash_desk(
    c: &mut Canvas,
    zone: u16,
    f: &Frame,
    block: Block,
    on_top: Option<Block>,
) -> Vec<(i32, i32)> {
    let ue = entry_u(c, zone, f);
    for side in [1, -1] {
        let u = ue + 2 * side;
        let cells: Vec<(i32, i32)> = (1..=3).map(|v| f.world(u, v)).collect();
        if !cells
            .iter()
            .all(|&(x, z)| c.is_free(x, z) && c.zone_of(x, z) == zone)
        {
            continue;
        }
        for (i, &(x, z)) in cells.iter().enumerate() {
            c.put(x, 1, z, block);
            if i == 1 {
                if let Some(item) = on_top {
                    c.put(x, 2, z, item);
                }
            }
        }
        for v in 1..=3 {
            let (x, z) = f.world(u + side, v);
            c.keep(x, z);
        }
        return cells;
    }
    Vec::new()
}

pub(super) fn shop(c: &mut Canvas, zone: u16, goods: Goods, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    let wares = wares(goods);
    cash_desk(
        c,
        zone,
        &f,
        counter_block(goods, ctx),
        wares.first().copied().or(Some(LANTERN)),
    );
    match goods {
        Goods::Salon => return salon(c, zone, &f, ctx),
        Goods::Furniture => return showroom(c, zone, &f, ctx),
        _ => {}
    }
    let stock = stock(goods);
    let tall = c.headroom() >= 3;

    // Shelving along the walls; low enough under windows to leave them clear.
    for ((x, z), n) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed);
        // A tailor's loom here and there between the clothes.
        if goods == Goods::Clothes && h.is_multiple_of(9) {
            c.put_with(x, 1, z, facing_block(LOOM, n));
            continue;
        }
        let low = if goods == Goods::Books {
            None
        } else {
            Some(pick(stock, h))
        };
        let placed = match low {
            Some(b) => c.put(x, 1, z, b),
            None => c.put_with(x, 1, z, book_shelf(n, h)),
        };
        if !placed {
            continue;
        }
        if tall && !c.window_behind(x, z, n) {
            match low {
                Some(_) => c.put(x, 2, z, pick(stock, h >> 16)),
                None => c.put_with(x, 2, z, book_shelf(n, h >> 16)),
            };
        } else if !wares.is_empty() && h.is_multiple_of(3) {
            c.put(x, 2, z, pick(wares, h >> 24));
        }
    }

    // Display tables in rows from the front to the back, the entry left as an aisle.
    let depth = c.depth(zone);
    let ue = entry_u(c, zone, &f);
    for (x, z) in c.cells(zone) {
        if depth.get(x, z) < 3 || !c.is_free(x, z) {
            continue;
        }
        let (u, v) = f.local(x, z);
        if (u - ue).rem_euclid(4) != 2 || v % 6 == 0 {
            continue;
        }
        let h = mix(x, z, ctx.seed ^ 0x5A0E);
        if goods == Goods::Books {
            c.put_with(x, 1, z, top_slab(ctx.wood.slab));
            if h.is_multiple_of(2) {
                c.put(x, 2, z, LECTERN);
            }
            continue;
        }
        c.put(x, 1, z, pick(stock, h));
        if !wares.is_empty() && h.is_multiple_of(2) {
            c.put(x, 2, z, pick(wares, h >> 8));
        }
    }
    // Spare stock in chests at the back.
    for ((x, z), _) in c.wall_cells(zone) {
        if f.local(x, z).1 >= f.depth - 2 && mix(x, z, ctx.seed ^ 0xC4E5).is_multiple_of(11) {
            chest(c, x, z, LootTheme::Mixed, ctx.salt);
        }
    }
}

/// Hairdresser: chairs facing mirrors along the walls, basins in a corner.
fn salon(c: &mut Canvas, zone: u16, f: &Frame, ctx: &FloorCtx) {
    for ((x, z), n) in c.wall_cells(zone) {
        let (u, v) = f.local(x, z);
        if v < 2 || (u + v) % 2 != 0 {
            continue;
        }
        let (sx, sz) = (x + n.0, z + n.1);
        if !c.is_free(sx, sz) {
            continue;
        }
        if mix(x, z, ctx.seed).is_multiple_of(4) {
            c.put(x, 1, z, WATER_CAULDRON);
        } else {
            c.put_with(x, 1, z, top_slab(SMOOTH_QUARTZ_SLAB));
            c.mount(x, 2, z, n, LIGHT_GRAY_STAINED_GLASS);
        }
        c.put_with(sx, 1, sz, seat(QUARTZ_STAIRS, (-n.0, -n.1)));
    }
    plant(c, f, 0, 1, ctx.seed);
}

/// Furniture store: little room settings standing about the floor.
fn showroom(c: &mut Canvas, zone: u16, f: &Frame, ctx: &FloorCtx) {
    for ((x, z), n) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed);
        if h.is_multiple_of(3) {
            c.put_with(x, 1, z, book_shelf(n, h));
        } else if h % 3 == 1 {
            c.put(x, 1, z, BARREL);
        }
    }
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if depth.get(x, z) < 3 || u % 5 != 2 || v % 5 != 2 {
            continue;
        }
        match mix(x, z, ctx.seed ^ 0xF0) % 3 {
            0 => {
                table_set(c, f, u, v, ctx.wood, &[(-1, 0), (1, 0), (0, 1)]);
            }
            1 => {
                super::canvas::bed(c, (x, z), f.dir(0, 1), RED_BED_NORTH_HEAD);
            }
            _ => {
                for (du, dv) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (cx, cz) = f.world(u + du, v + dv);
                    if c.is_free(cx, cz) {
                        c.put(cx, 1, cz, RED_CARPET);
                    }
                }
            }
        }
    }
}

pub(super) fn supermarket(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    let ue = entry_u(c, zone, &f);
    let tall = c.headroom() >= 3;

    // Checkout lanes beside the entry: counter, cashier, walkway.
    for side in [1, -1] {
        for lane in 0..3 {
            let u = ue + side * (2 + lane * 3);
            let cells: Vec<(i32, i32)> = (2..=4).map(|v| f.world(u, v)).collect();
            if !cells.iter().all(|&(x, z)| c.is_free(x, z)) {
                break;
            }
            for (i, &(x, z)) in cells.iter().enumerate() {
                c.put_with(x, 1, z, top_slab(SMOOTH_STONE_SLAB));
                if i == 1 {
                    c.put(x, 2, z, DAYLIGHT_DETECTOR);
                }
            }
            let (sx, sz) = f.world(u + side, 3);
            c.put_with(sx, 1, sz, seat(ctx.wood.stairs, f.dir(-side, 0)));
        }
    }

    // Fridges along the back wall, shelving along the others.
    for ((x, z), n) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed);
        if f.local(x, z).1 < 6 {
            continue;
        }
        if n == f.front() {
            c.put(x, 1, z, WHITE_CONCRETE);
            if tall {
                c.put(x, 2, z, LIGHT_BLUE_STAINED_GLASS);
            }
        } else {
            c.put(x, 1, z, pick(&[BARREL, HAY_BALE, BARREL, PUMPKIN], h));
            if tall && !c.window_behind(x, z, n) {
                c.put(x, 2, z, pick(&[BARREL, MELON, BARREL], h >> 8));
            }
        }
    }

    // Fruit and vegetable bins past the checkouts, then aisles of double shelving.
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        if depth.get(x, z) < 3 || !c.is_free(x, z) {
            continue;
        }
        let (u, v) = f.local(x, z);
        let lane = (u - ue).rem_euclid(4);
        let h = mix(x, z, ctx.seed ^ 0xA15E);
        if v == 7 && lane == 2 {
            c.put(x, 1, z, pick(&[HAY_BALE, MELON, PUMPKIN, COMPOSTER], h));
        } else if v >= 9 && (lane == 2 || lane == 3) && v % 10 != 0 {
            c.put(
                x,
                1,
                z,
                pick(&[BARREL, BARREL, HAY_BALE, CHISELLED_BOOKSHELF], h),
            );
            if tall {
                c.put(x, 2, z, pick(&[BARREL, MELON, PUMPKIN, BARREL], h >> 8));
            }
        }
    }
}

pub(super) fn eatery(c: &mut Canvas, zone: u16, kind: Eatery, ctx: &FloorCtx) {
    c.focus(zone);
    let Some(f) = c.frame(zone) else {
        return;
    };
    let cells = c.area(zone);
    // A kitchen behind a wall at the back of anything bigger than a corner cafe.
    let kitchen_row = (f.depth >= 13 && cells >= 90).then_some(f.depth - 5);
    if let Some(row) = kitchen_row {
        if let Some(kitchen) = c.split_back(zone, &f, row) {
            furnish_kitchen(c, kitchen, ctx);
            c.focus(zone);
        }
    }
    let service_v = kitchen_row.map_or(f.depth - 3, |r| r - 2);

    // Counter across the room in front of the kitchen or the back wall.
    let ue = entry_u(c, zone, &f);
    let half = (f.width / 4).clamp(1, 4);
    let mid = f.width / 2;
    let counter = match kind {
        Eatery::Bar => SPRUCE_PLANKS,
        Eatery::FastFood => POLISHED_ANDESITE,
        _ => ctx.wood.planks,
    };
    for u in mid - half..=mid + half {
        let (x, z) = f.world(u, service_v);
        if c.put(x, 1, z, counter) {
            let h = mix(x, z, ctx.seed);
            let item = match kind {
                Eatery::Cafe => pick(&[CAKE, EMPTY_FLOWER_POT, BREWING_STAND], h),
                Eatery::Bar => pick(&[BREWING_STAND, EMPTY_FLOWER_POT, LANTERN], h),
                _ => pick(&[EMPTY_FLOWER_POT, LANTERN, CAKE], h),
            };
            if h.is_multiple_of(3) {
                c.put(x, 2, z, item);
            }
            // Staff walk behind the counter.
            let (bx, bz) = f.world(u, service_v + 1);
            c.keep(bx, bz);
        }
    }
    // Barrels and stools at a bar.
    if kind == Eatery::Bar {
        for u in mid - half..=mid + half {
            let (sx, sz) = f.world(u, service_v - 1);
            if u % 2 == 0 {
                c.put_with(sx, 1, sz, seat(ctx.wood.stairs, f.back()));
            }
        }
        for ((x, z), _) in c.wall_cells(zone) {
            if f.local(x, z).1 > service_v {
                c.put(x, 1, z, BARREL);
            }
        }
    }

    // Tables between the door and the counter.
    let depth = c.depth(zone);
    let mut tables = Vec::new();
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if v < 2 || v > service_v - 2 || depth.get(x, z) < 2 {
            continue;
        }
        // Two cells of aisle between neighbouring tables and their chairs.
        if (u - ue).rem_euclid(5) != 2 {
            continue;
        }
        let placed = match kind {
            Eatery::Cafe if v % 3 == 2 => table_set(c, &f, u, v, ctx.wood, &[(-1, 0), (1, 0)]),
            Eatery::Restaurant if v % 4 == 2 => {
                table_set(c, &f, u, v, ctx.wood, &[(-1, 0), (1, 0), (0, 1)])
            }
            // A long table with benches down both sides.
            Eatery::Bar | Eatery::FastFood if v % 4 == 2 => {
                let (x2, z2) = f.world(u, v + 1);
                c.is_free(x2, z2)
                    && table_set(c, &f, u, v, ctx.wood, &[(-1, 0), (1, 0)])
                    && table_set(c, &f, u, v + 1, ctx.wood, &[(-1, 0), (1, 0)])
            }
            _ => false,
        };
        if placed {
            tables.push((x, z));
        }
    }
    // Greenery in the front corners, light over the tables.
    plant(c, &f, 0, 1, ctx.seed);
    plant(c, &f, f.width - 1, 1, ctx.seed ^ 1);
    for (x, z) in tables {
        if c.headroom() >= 4 {
            c.put_with(x, c.headroom(), z, hanging_lantern());
        } else if kind == Eatery::Restaurant {
            c.put(x, 2, z, LANTERN);
        }
    }
}

/// Stoves, sinks and worktops round the walls, a prep table in the middle.
fn furnish_kitchen(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    c.focus(zone);
    for ((x, z), n) in c.wall_cells(zone) {
        let block = match mix(x, z, ctx.seed ^ 0x4C17) % 5 {
            0 => facing_block(SMOKER, n),
            1 => facing_block(FURNACE, n),
            2 => BlockWithProperties::simple(WATER_CAULDRON),
            3 => BlockWithProperties::simple(CRAFTING_TABLE),
            _ => BlockWithProperties::simple(BARREL),
        };
        c.put_with(x, 1, z, block);
    }
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        if depth.get(x, z) >= 3 && mix(x, z, ctx.seed).is_multiple_of(2) {
            c.put_with(x, 1, z, top_slab(SMOOTH_STONE_SLAB));
        }
    }
    for (x, z) in c.cells(zone) {
        if mix(x, z, ctx.seed ^ 0xF00D).is_multiple_of(9) {
            chest(c, x, z, LootTheme::Food, ctx.salt);
        }
    }
}

pub(super) fn office(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    let cells = c.area(zone);
    // Reception by the door downstairs.
    if ctx.floor == 0 && cells >= 60 {
        cash_desk(c, zone, &f, ctx.wood.planks, Some(EMPTY_FLOWER_POT));
    }
    // A meeting room behind a wall at the back of a big floor.
    if f.depth >= 14 && cells >= 160 {
        if let Some(meeting) = c.split_back(zone, &f, f.depth - 6) {
            meeting_room(c, meeting, ctx);
            c.focus(zone);
        }
    }

    // Islands of four desks with aisles all round.
    let depth = c.depth(zone);
    let ue = entry_u(c, zone, &f);
    for (x, z) in c.cells(zone) {
        if depth.get(x, z) < 2 || !c.is_free(x, z) {
            continue;
        }
        let (u, v) = f.local(x, z);
        if v < 3 || !matches!(v % 4, 1 | 2) {
            continue;
        }
        match (u - ue).rem_euclid(6) {
            2 | 3 => {
                c.put_with(x, 1, z, top_slab(ctx.wood.slab));
                c.put(x, 2, z, GRAY_STAINED_GLASS_PANE);
            }
            1 => {
                c.put_with(x, 1, z, seat(ctx.wood.stairs, f.dir(1, 0)));
            }
            4 => {
                c.put_with(x, 1, z, seat(ctx.wood.stairs, f.dir(-1, 0)));
            }
            _ => {}
        }
    }
    // Filing cabinets and plants along the walls, water coolers near the front.
    for ((x, z), n) in c.wall_cells(zone) {
        let h = mix(x, z, ctx.seed);
        if f.local(x, z).1 <= 3 && h.is_multiple_of(7) {
            if c.put(x, 1, z, WHITE_CONCRETE) && c.headroom() >= 3 {
                c.put(x, 2, z, LIGHT_BLUE_STAINED_GLASS);
            }
            continue;
        }
        match h % 8 {
            0 => {
                c.put_with(x, 1, z, book_shelf(n, h));
            }
            1 => {
                c.put(x, 1, z, BARREL);
            }
            2 => {
                c.put(x, 1, z, pick(&[AZALEA, FLOWERING_AZALEA], h >> 8));
            }
            _ => {}
        }
    }
}

/// One long table with chairs down both sides.
fn meeting_room(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    c.focus(zone);
    let Some(f) = c.long_frame(zone) else {
        return;
    };
    let mid = f.width / 2;
    for v in 2..f.depth - 2 {
        table_set(c, &f, mid, v, ctx.wood, &[(-1, 0), (1, 0)]);
    }
    let (x, z) = f.world(mid, f.depth - 1);
    c.put(x, 1, z, pick(&[AZALEA, FLOWERING_AZALEA], ctx.seed));
}

pub(super) fn bank(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    // Teller counter across the hall with a grille on top, a gap at one end for staff.
    let row = (f.depth / 3).clamp(4, 6);
    let mut tellers = Vec::new();
    for u in 1..f.width - 2 {
        let (x, z) = f.world(u, row);
        if c.put(x, 1, z, POLISHED_ANDESITE) {
            if c.headroom() >= 3 {
                c.put(x, 2, z, IRON_BARS);
            }
            if u % 3 == 1 {
                tellers.push(u);
            }
        }
    }
    for u in tellers {
        let (x, z) = f.world(u, row + 1);
        c.put_with(x, 1, z, seat(ctx.wood.stairs, f.front()));
    }
    // Waiting chairs along the side walls in front of the counter.
    for ((x, z), n) in c.wall_cells(zone) {
        let v = f.local(x, z).1;
        if v >= 1 && v < row - 1 && n != f.back() {
            c.put_with(x, 1, z, seat(ctx.wood.stairs, n));
        } else if v > row + 2 {
            // The vault: chests and cabinets along the back.
            if mix(x, z, ctx.seed).is_multiple_of(3) {
                chest(c, x, z, LootTheme::Valuables, ctx.salt);
            } else {
                c.put(x, 1, z, BARREL);
            }
        }
    }
    plant(c, &f, 0, 1, ctx.seed);
}

pub(super) fn workshop(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    for ((x, z), n) in c.wall_cells(zone) {
        if f.local(x, z).1 < 2 {
            continue;
        }
        match mix(x, z, ctx.seed ^ 0x3077) % 7 {
            0 => c.put(x, 1, z, CRAFTING_TABLE),
            1 => c.put(x, 1, z, SMITHING_TABLE),
            2 => c.put(x, 1, z, ANVIL),
            3 => c.put_with(x, 1, z, grindstone(n)),
            4 => c.put(x, 1, z, BARREL),
            5 => chest(c, x, z, LootTheme::Tools, ctx.salt),
            _ => c.put_with(x, 1, z, facing_block(FURNACE, n)),
        };
    }
    // A lift with a vehicle-sized gap in the middle of a big bay.
    let depth = c.depth(zone);
    for (x, z) in c.cells(zone) {
        let (u, v) = f.local(x, z);
        if depth.get(x, z) >= 3 && v % 6 == 3 && (u % 4 == 1 || u % 4 == 3) {
            c.put(x, 1, z, IRON_BLOCK);
        }
    }
}
