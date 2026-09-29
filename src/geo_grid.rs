//! Reader for the bundled zstd-tiled world grids in `assets/climate/*.grid`.

const HEADER: usize = 24;

pub struct TiledGrid {
    data: &'static [u8],
    cell_bytes: usize,
    tile: usize,
    cols: usize,
    rows: usize,
    cell_deg: f64,
    frames_at: usize,
}

impl TiledGrid {
    pub fn parse(data: &'static [u8]) -> Option<Self> {
        if data.len() < HEADER || &data[..4] != b"AGRD" || data[4] != 1 {
            return None;
        }
        let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        let cell_bytes = data[5] as usize;
        let tile = u16_at(6);
        let (cols, rows) = (u32_at(8), u32_at(12));
        let cell_deg = f64::from_le_bytes(data[16..24].try_into().unwrap());
        if !matches!(cell_bytes, 1 | 2) || tile == 0 || cols % tile != 0 || rows % tile != 0 {
            return None;
        }
        let grid = Self {
            data,
            cell_bytes,
            tile,
            cols,
            rows,
            cell_deg,
            frames_at: HEADER + 4 * ((cols / tile) * (rows / tile) + 1),
        };
        if grid.frames_at > data.len() {
            return None;
        }
        let last = grid.offset(grid.tile_count());
        (grid.frames_at + last == data.len()).then_some(grid)
    }

    pub fn tile_count(&self) -> usize {
        (self.cols / self.tile) * (self.rows / self.tile)
    }

    fn offset(&self, i: usize) -> usize {
        let at = HEADER + 4 * i;
        u32::from_le_bytes(self.data[at..at + 4].try_into().unwrap()) as usize
    }

    /// Cells per degree along either axis.
    pub fn cells_per_degree(&self) -> f64 {
        1.0 / self.cell_deg
    }

    /// Cell holding a point, in fractional cell units from the north-west corner.
    pub fn position(&self, lat: f64, lon: f64) -> (f64, f64) {
        ((lon + 180.0) / self.cell_deg, (90.0 - lat) / self.cell_deg)
    }

    /// Integer cell for a fractional position; longitude wraps, latitude clamps.
    pub fn cell(&self, col: f64, row: f64) -> (usize, usize) {
        let c = (col.floor() as i64).rem_euclid(self.cols as i64) as usize;
        let r = (row.floor() as i64).clamp(0, self.rows as i64 - 1) as usize;
        (c, r)
    }

    pub fn tile_of(&self, (col, row): (usize, usize)) -> usize {
        (row / self.tile) * (self.cols / self.tile) + col / self.tile
    }

    /// Index of a cell inside its decoded tile.
    pub fn index_in_tile(&self, (col, row): (usize, usize)) -> usize {
        (row % self.tile) * self.tile + col % self.tile
    }

    /// One tile's cells as stored (little endian); `None` for an all-zero tile.
    pub fn decode(&self, tile: usize) -> Option<Vec<u8>> {
        let (start, end) = (self.offset(tile), self.offset(tile + 1));
        if start == end {
            return None;
        }
        let frame = &self.data[self.frames_at + start..self.frames_at + end];
        let cap = self.tile * self.tile * self.cell_bytes;
        zstd::bulk::decompress(frame, cap)
            .ok()
            .filter(|v| v.len() == cap)
    }

    /// Value at `index` of a decoded tile.
    pub fn value(&self, tile: &[u8], index: usize) -> u16 {
        match self.cell_bytes {
            1 => u16::from(tile[index]),
            _ => u16::from_le_bytes([tile[2 * index], tile[2 * index + 1]]),
        }
    }
}
