//! The Hollywood Sign at one block per metre: nine sheet-steel letters on their frames and legs,
//! each standing on its own patch of Mount Lee.

use std::collections::BTreeMap;

use super::schematic::PalettizedSchematic;
use crate::block_definitions::*;
use crate::element_processing::structure_kit::with_props;

/// Model block whose north-west corner is the mapped Hollywood Sign node.
pub const ANCHOR_X: i32 = 54;
pub const ANCHOR_Z: i32 = 5;
/// Model layer of the letters' bottom edge; the legs reach below it into the slope.
pub const GROUND: i32 = 8;
const SIDE_X: i32 = 110;
const SIDE_Z: i32 = 18;
/// Rows of a letter, 13.7 m.
const ROWS: usize = 14;

/// The letters' mapped baselines, replaced by the model.
pub const LETTER_WAYS: &[(&str, u64)] = &[
    ("way", 125_433_362),
    ("way", 125_433_363),
    ("way", 125_433_364),
    ("way", 125_433_365),
    ("way", 125_433_366),
    ("way", 125_433_367),
    ("way", 125_433_369),
    ("way", 125_433_370),
    ("way", 125_433_372),
];

// Letterforms measured off photographs, top row first: `#` a full panel, `/` `\` a panel cut away
// above (filled low right, low left), `r` `7` cut away below (filled high left, high right).
const H: [&str; ROWS] = [
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "##########",
    "##########",
    "##########",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
];
const O: [&str; ROWS] = [
    "./######\\.",
    "/########\\",
    "##########",
    "###r..7###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###\\../###",
    "##########",
    "7########r",
    ".7######r.",
];
const L: [&str; ROWS] = [
    "###......",
    "###......",
    "###......",
    "###......",
    "###......",
    "###......",
    "###......",
    "###......",
    "###......",
    "###......",
    "###......",
    "#########",
    "#########",
    "#########",
];
const Y: [&str; ROWS] = [
    "####...####",
    "####...####",
    "7###\\./###r",
    ".####.####.",
    ".####.####.",
    ".7#######r.",
    "..#######..",
    "..7#####r..",
    "...#####...",
    "...#####...",
    "...#####...",
    "...#####...",
    "...#####...",
    "...#####...",
];
const W: [&str; ROWS] = [
    "###..##..###",
    "###\\.##./###",
    "####.##.####",
    "####.##.####",
    "7##########r",
    ".##########.",
    ".##########.",
    ".##########.",
    ".##########.",
    ".##########.",
    ".####r7####.",
    ".7###..###r.",
    "..###..###..",
    "..###..###..",
];
const D: [&str; ROWS] = [
    "########\\.",
    "#########\\",
    "##########",
    "###...7###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###....###",
    "###.../###",
    "##########",
    "#########r",
    "########r.",
];

/// Each letter's west edge and front face, in metres east and south of the anchor, from the mapped
/// baselines spread to the sign's 107 m, and whether its frame fills its whole box.
const LETTERS: [(&[&str; ROWS], i32, i32, bool); 9] = [
    (&H, -53, 8, true),
    (&O, -40, 11, true),
    (&L, -27, 9, false),
    (&L, -17, 7, false),
    (&Y, -7, 6, true),
    (&W, 5, 1, true),
    (&O, 18, -1, true),
    (&O, 31, -3, true),
    (&D, 44, 2, true),
];
/// Layers of the frame's horizontal girts, and how far the legs reach under the letters.
const GIRTS: [i32; 5] = [2, 5, 8, 11, 13];
const LEG_DEPTH: i32 = 8;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Panel,
    /// A stair-cut panel: its full half on the east, and whether that half is the upper one.
    Cut {
        east: bool,
        top: bool,
    },
    Bars,
}

#[derive(Default)]
struct Sign {
    cells: BTreeMap<(i32, i32, i32), Part>,
}

impl Sign {
    fn set(&mut self, x: i32, y: i32, z: i32, part: Part) {
        self.cells
            .insert((ANCHOR_X + x, GROUND + y, ANCHOR_Z + z), part);
    }
}

pub fn build() -> PalettizedSchematic {
    let mut sign = Sign::default();
    for &(rows, west, front, boxed) in &LETTERS {
        letter(&mut sign, rows, west, front, boxed);
    }
    finish(sign)
}

fn letter(sign: &mut Sign, rows: &[&str; ROWS], west: i32, front: i32, boxed: bool) {
    let width = rows[0].len() as i32;
    for (i, row) in rows.iter().enumerate() {
        let y = (ROWS - 1 - i) as i32;
        for (dx, ch) in row.chars().enumerate() {
            let part = match ch {
                '#' => Part::Panel,
                '/' => Part::Cut {
                    east: true,
                    top: false,
                },
                '\\' => Part::Cut {
                    east: false,
                    top: false,
                },
                '7' => Part::Cut {
                    east: true,
                    top: true,
                },
                'r' => Part::Cut {
                    east: false,
                    top: true,
                },
                _ => continue,
            };
            sign.set(west + dx as i32, y, front, part);
        }
    }
    // The steel frame behind the panels: posts at both ends and the middle, girts across.
    let back = front - 1;
    let posts = [1, width / 2, width - 2];
    for y in 1..ROWS as i32 {
        for dx in 0..width {
            let row = rows[ROWS - 1 - y as usize].as_bytes();
            let behind_panel = row[dx as usize] != b'.';
            let post = posts.contains(&dx) && (boxed || behind_panel);
            let girt = GIRTS.contains(&y) && (boxed || behind_panel);
            if post || girt {
                sign.set(west + dx, y, back, Part::Bars);
            }
        }
    }
    // Legs under the bottom edge, down into the hillside.
    let bottom: Vec<i32> = (0..width)
        .filter(|&dx| rows[ROWS - 1].as_bytes()[dx as usize] != b'.')
        .collect();
    let (first, last) = (bottom[0], bottom[bottom.len() - 1]);
    for dx in [first + 1, last - 1] {
        for y in -LEG_DEPTH..0 {
            sign.set(west + dx, y, front, Part::Bars);
        }
    }
}

/// Bars join each other and the full panels they back onto: north, south, east, west.
fn joins(sign: &Sign, (x, y, z): (i32, i32, i32)) -> [bool; 4] {
    [(0, -1), (0, 1), (1, 0), (-1, 0)].map(|(dx, dz)| {
        matches!(
            sign.cells.get(&(x + dx, y, z + dz)),
            Some(Part::Bars | Part::Panel)
        )
    })
}

fn resolve(part: Part, sides: [bool; 4]) -> BlockWithProperties {
    match part {
        Part::Panel => BlockWithProperties::simple(QUARTZ_BLOCK),
        Part::Cut { east, top } => with_props(
            QUARTZ_STAIRS,
            &[
                ("facing", if east { "east" } else { "west" }),
                ("half", if top { "top" } else { "bottom" }),
                ("shape", "straight"),
            ],
        ),
        Part::Bars => {
            let side = |b: bool| if b { "true" } else { "false" };
            with_props(
                IRON_BARS,
                &[
                    ("north", side(sides[0])),
                    ("south", side(sides[1])),
                    ("east", side(sides[2])),
                    ("west", side(sides[3])),
                ],
            )
        }
    }
}

fn finish(sign: Sign) -> PalettizedSchematic {
    let mut palette: Vec<BlockWithProperties> = Vec::new();
    let mut slots: BTreeMap<(Part, [bool; 4]), u8> = BTreeMap::new();
    let mut voxels = Vec::with_capacity(sign.cells.len());
    for (&(x, y, z), &part) in &sign.cells {
        debug_assert!((0..SIDE_X).contains(&x) && (0..SIDE_Z).contains(&z) && y >= 0);
        let sides = if part == Part::Bars {
            joins(&sign, (x, y, z))
        } else {
            [false; 4]
        };
        let slot = *slots.entry((part, sides)).or_insert_with(|| {
            palette.push(resolve(part, sides));
            u8::try_from(palette.len() - 1).expect("the sign uses few block states")
        });
        voxels.push((x as i16, y as i16, z as i16, slot));
    }
    PalettizedSchematic {
        width: SIDE_X,
        length: SIDE_Z,
        palette,
        voxels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_are_45_feet_and_the_sign_350_feet() {
        let model = build();
        let panels: Vec<_> = model
            .voxels
            .iter()
            .filter(|v| model.palette[v.3 as usize].block != IRON_BARS)
            .collect();
        let (lo, hi) = (
            panels.iter().map(|v| v.1).min().unwrap(),
            panels.iter().map(|v| v.1).max().unwrap(),
        );
        assert_eq!((lo, hi), (GROUND as i16, GROUND as i16 + 13));
        let (west, east) = (
            panels.iter().map(|v| v.0).min().unwrap(),
            panels.iter().map(|v| v.0).max().unwrap(),
        );
        assert_eq!(east - west + 1, 107);
    }

    #[test]
    fn letterforms_are_whole() {
        for rows in [H, O, L, Y, W, D] {
            let width = rows[0].len();
            assert!(rows.iter().all(|r| r.len() == width));
            assert!(rows
                .iter()
                .all(|r| r.chars().all(|c| "#./\\7r".contains(c))));
        }
    }

    #[test]
    fn letters_stand_apart_so_each_finds_its_own_ground() {
        let mut spans: Vec<(i32, i32)> = LETTERS
            .iter()
            .map(|(rows, west, ..)| (*west, west + rows[0].len() as i32 - 1))
            .collect();
        spans.sort_unstable();
        assert!(spans.windows(2).all(|w| w[1].0 - w[0].1 >= 2));
    }
}
