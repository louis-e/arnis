//! Cloud-optimised point clouds (COPC) read over HTTP, in part.
//!
//! A COPC file is a LAZ 1.4 file whose points are stored as an octree: each node
//! holds a thinned sample of the points in its cube, and the deeper levels add
//! the rest. The header and the `copc` info record give the cube; hierarchy
//! pages list every node's place in the file. So an area is read as: the first
//! 64 KiB (header and records), the hierarchy pages over the area, then only
//! the nodes whose squares meet the area, and only down to the level that gives
//! the density asked for. Neighbouring nodes are fetched in one request, and each
//! node is its own LAZ chunk, decoded on its own.

use super::crs::Affine;
use super::fetch::{self, Failure};
use super::source::Rect;
use laz::record::{LayeredPointRecordDecompressor, RecordDecompressor};
use laz::{DecompressionSelection, LazVlr};

/// The parts of the header a reader needs.
pub struct Header {
    pub layout: PointLayout,
    /// Extent of the points: west, south, east, north.
    pub bounds: Rect,
    /// The file's grid as OGC WKT, where it names one.
    pub wkt: Option<String>,
    info: Info,
    laz: LazVlr,
}

/// How to read one point record.
#[derive(Clone, Copy, Debug)]
pub struct PointLayout {
    pub format: u8,
    pub record_len: usize,
    pub scale: [f64; 3],
    pub offset: [f64; 3],
}

/// The shortest record of each LAS point format, 0 to 10.
const MIN_RECORD: [usize; 11] = [20, 28, 26, 34, 57, 63, 30, 36, 38, 59, 67];

impl PointLayout {
    /// Whether the records are of a known format and long enough to hold it,
    /// as reading them takes for granted.
    pub fn check(&self) -> Result<(), String> {
        match MIN_RECORD.get(usize::from(self.format)) {
            Some(&min) if self.record_len >= min => Ok(()),
            _ => Err(format!(
                "point format {} in records of {} bytes",
                self.format, self.record_len
            )),
        }
    }
}

/// Deepest octree level read: a kilometre cube's nodes there are a sixteenth
/// of a millimetre across, so anything deeper is not a real hierarchy.
pub const MAX_LEVEL: i32 = 24;
/// Most points one node may hold, and the most bytes it may take compressed
/// and decoded.
const MAX_NODE_POINTS: u64 = 10_000_000;
const MAX_NODE_BYTES: u64 = 256 << 20;
/// Largest hierarchy page read.
const MAX_PAGE_BYTES: u64 = 16 << 20;

/// The `copc` info record: the octree's root cube and first hierarchy page.
#[derive(Clone, Copy, Debug)]
pub struct Info {
    pub center: [f64; 3],
    pub halfsize: f64,
    pub root_offset: u64,
    pub root_size: u64,
}

/// One octree node with points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub level: i32,
    pub x: i32,
    pub y: i32,
    pub offset: u64,
    pub size: u64,
    pub count: u64,
}

/// What a read came to.
#[derive(Default, Debug)]
pub struct ReadStats {
    pub level: i32,
    pub nodes: usize,
    pub requests: usize,
    pub bytes: u64,
    pub points: u64,
}

const HEAD: u64 = 65_536;

fn parse_header(h: &[u8]) -> Result<(Header, u64), String> {
    if h.len() < 375 || &h[..4] != b"LASF" {
        return Err("not a LAS/LAZ file".into());
    }
    let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());
    let f64_at = |o: usize| f64::from_le_bytes(h[o..o + 8].try_into().unwrap());
    let header_size = u64::from(u16_at(94));
    let n_vlrs = u32_at(100);
    let format = h[104] & 0x3F;
    let record_len = usize::from(u16_at(105));
    let scale = [f64_at(131), f64_at(139), f64_at(147)];
    let offset = [f64_at(155), f64_at(163), f64_at(171)];
    // Max x, min x, max y, min y.
    let bounds = (f64_at(187), f64_at(203), f64_at(179), f64_at(195));
    let mut info = None;
    let mut laz = None;
    let mut wkt = None;
    let mut pos = header_size as usize;
    for _ in 0..n_vlrs {
        if pos + 54 > h.len() {
            // The records run past what was read: ask for this much.
            return Err(format!("need {}", pos + 54 + 65_536));
        }
        let user = String::from_utf8_lossy(&h[pos + 2..pos + 18]);
        let user = user.trim_end_matches('\0');
        let id = u16_at(pos + 18);
        let len = usize::from(u16_at(pos + 20));
        let data = pos + 54;
        if data + len > h.len() {
            return Err(format!("need {}", data + len));
        }
        if user == "copc" && id == 1 && len >= 72 {
            info = Some(Info {
                center: [f64_at(data), f64_at(data + 8), f64_at(data + 16)],
                halfsize: f64_at(data + 24),
                root_offset: u64_at(data + 40),
                root_size: u64_at(data + 48),
            });
        } else if user == "laszip encoded" && id == 22204 {
            laz = Some(LazVlr::read_from(&h[data..data + len]).map_err(|e| e.to_string())?);
        } else if user == "LASF_Projection" && id == 2112 {
            let text = String::from_utf8_lossy(&h[data..data + len]);
            wkt = Some(text.trim_end_matches('\0').to_string());
        }
        pos = data + len;
    }
    let info = info.ok_or("no COPC info record")?;
    let laz = laz.ok_or("no LAZ record")?;
    if format < 6 {
        return Err(format!("COPC point format {format}"));
    }
    let layout = PointLayout {
        format,
        record_len,
        scale,
        offset,
    };
    layout.check()?;
    if info.root_size > MAX_PAGE_BYTES {
        return Err(format!("a hierarchy page of {} bytes", info.root_size));
    }
    Ok((
        Header {
            layout,
            bounds,
            wkt,
            info,
            laz,
        },
        pos as u64,
    ))
}

/// The square a node covers: west, south, east, north.
pub fn node_rect(info: &Info, level: i32, x: i32, y: i32) -> Rect {
    let size = 2.0 * info.halfsize / f64::from(1u32 << level.clamp(0, 30));
    let (x0, y0) = (
        info.center[0] - info.halfsize,
        info.center[1] - info.halfsize,
    );
    (
        x0 + f64::from(x) * size,
        y0 + f64::from(y) * size,
        x0 + f64::from(x + 1) * size,
        y0 + f64::from(y + 1) * size,
    )
}

fn overlap(a: Rect, b: Rect) -> f64 {
    let w = a.2.min(b.2) - a.0.max(b.0);
    let h = a.3.min(b.3) - a.1.max(b.1);
    if w > 0.0 && h > 0.0 {
        w * h
    } else {
        0.0
    }
}

/// One hierarchy page: its nodes with points, and the child pages it points to
/// as (level, x, y, offset, size).
type Page = (Vec<Node>, Vec<(i32, i32, i32, u64, u64)>);

/// The entries of a hierarchy page, or what is out of range in them: a level
/// past [`MAX_LEVEL`], a node past the size limits, a range past the end of
/// any file.
fn parse_page(d: &[u8]) -> Result<Page, String> {
    let mut nodes = Vec::new();
    let mut pages = Vec::new();
    for e in d.as_chunks::<32>().0 {
        let i32_at = |o: usize| i32::from_le_bytes(e[o..o + 4].try_into().unwrap());
        let (level, x, y) = (i32_at(0), i32_at(4), i32_at(8));
        let offset = u64::from_le_bytes(e[16..24].try_into().unwrap());
        let size = i32_at(24);
        let count = i32_at(28);
        if size <= 0 || !(count == -1 || count > 0) {
            continue;
        }
        let size = size as u64;
        if !(0..=MAX_LEVEL).contains(&level) || offset.checked_add(size).is_none() {
            return Err(format!(
                "a hierarchy entry at level {level}, {size} bytes at {offset}"
            ));
        }
        if count == -1 {
            if size > MAX_PAGE_BYTES {
                return Err(format!("a hierarchy page of {size} bytes"));
            }
            pages.push((level, x, y, offset, size));
        } else {
            let count = count as u64;
            if count > MAX_NODE_POINTS || size > MAX_NODE_BYTES {
                return Err(format!("a node of {count} points in {size} bytes"));
            }
            nodes.push(Node {
                level,
                x,
                y,
                offset,
                size,
                count,
            });
        }
    }
    Ok((nodes, pages))
}

/// The shallowest level whose points, with all above it, give `target` points a
/// square metre, else the deepest there is. Each level's density is its points
/// over the squares its nodes cover inside the file's extent, so the choice is
/// the same for every window of a file and neighbouring areas of one tile are
/// never read at different densities.
pub fn choose_level(info: &Info, nodes: &[Node], bounds: Rect, target: f64) -> i32 {
    let deepest = nodes.iter().map(|n| n.level).max().unwrap_or(0).max(0);
    let mut points = vec![0.0f64; deepest as usize + 1];
    let mut area = vec![0.0f64; deepest as usize + 1];
    for n in nodes.iter().filter(|n| n.level >= 0) {
        points[n.level as usize] += n.count as f64;
        area[n.level as usize] += overlap(node_rect(info, n.level, n.x, n.y), bounds);
    }
    let mut density = 0.0;
    for level in 0..=deepest as usize {
        if area[level] > 0.0 {
            density += points[level] / area[level];
        }
        if density >= target {
            return level as i32;
        }
    }
    deepest
}

/// The nodes down to `level` whose squares meet the window.
pub fn select(info: &Info, nodes: &[Node], window: Rect, level: i32) -> Vec<Node> {
    nodes
        .iter()
        .filter(|n| n.level <= level && overlap(node_rect(info, n.level, n.x, n.y), window) > 0.0)
        .copied()
        .collect()
}

/// Nodes grouped into byte ranges to fetch together: neighbours in the file
/// closer than `max_gap` share a request, up to `max_len` a request.
pub fn merge_ranges(nodes: &[Node], max_gap: u64, max_len: u64) -> Vec<(u64, u64, Vec<Node>)> {
    let mut sorted = nodes.to_vec();
    sorted.sort_by_key(|n| n.offset);
    let mut out: Vec<(u64, u64, Vec<Node>)> = Vec::new();
    for n in sorted {
        if let Some((start, len, group)) = out.last_mut() {
            let end = *start + *len;
            if n.offset >= end && n.offset - end <= max_gap && n.offset + n.size - *start <= max_len
            {
                *len = n.offset + n.size - *start;
                group.push(n);
                continue;
            }
        }
        out.push((n.offset, n.size, vec![n]));
    }
    out
}

/// Where a file's points go: from its WKT (when it has one), the map from its
/// grid onto the area's, or `None` when they share one.
pub type Place<'a> = dyn Fn(Option<&str>) -> Result<Option<Affine>, String> + Sync + 'a;

/// Read the header, records and hierarchy of a COPC file over `window` (in the
/// area's grid; `place` says how the file's sits to it). Returns the window in
/// the file's grid too. `Missing` when the server has no such file, `Unusable`
/// when it is not one this reader can use.
pub fn open(
    client: &reqwest::blocking::Client,
    url: &str,
    window: Rect,
    place: &Place,
) -> Result<(Header, Vec<Node>, usize, Rect), Failure> {
    let mut head = fetch::get_range(client, url, 0, HEAD)?;
    let mut requests = 1;
    fetch::count_laser_bytes(head.len() as u64);
    let header = loop {
        match parse_header(&head) {
            Ok((h, _)) => break h,
            Err(e) => match e.strip_prefix("need ").and_then(|n| n.parse::<u64>().ok()) {
                Some(n) if n > head.len() as u64 && n < 16 * HEAD => {
                    head = fetch::get_range(client, url, 0, n).map_err(Failure::of_part)?;
                    requests += 1;
                    fetch::count_laser_bytes(head.len() as u64);
                }
                _ => return Err(Failure::Unusable(e)),
            },
        }
    };
    let window = match place(header.wkt.as_deref()).map_err(Failure::Unusable)? {
        Some(f) => f.inverse().rect(window),
        None => window,
    };
    let info = header.info;
    let mut nodes = Vec::new();
    let mut pages = vec![(0, 0, 0, info.root_offset, info.root_size)];
    while let Some((level, x, y, offset, size)) = pages.pop() {
        if level > 0 && overlap(node_rect(&info, level, x, y), window) <= 0.0 {
            continue;
        }
        let page = fetch::get_range(client, url, offset, size).map_err(Failure::of_part)?;
        requests += 1;
        fetch::count_laser_bytes(page.len() as u64);
        let (n, p) = parse_page(&page).map_err(Failure::Unusable)?;
        nodes.extend(n);
        pages.extend(p);
        if requests > 200 {
            return Err(Failure::Other("hierarchy too large".into()));
        }
    }
    let record = header.layout.record_len as u64;
    if let Some(n) = nodes.iter().find(|n| n.count * record > MAX_NODE_BYTES) {
        return Err(Failure::Unusable(format!(
            "a node of {} points of {record} bytes",
            n.count
        )));
    }
    Ok((header, nodes, requests, window))
}

/// Read the points of `window` at `target` points a square metre, handing each
/// decoded node's records to `visit`. Ranges are fetched and decoded on up to
/// `threads` threads at a time.
pub fn read(
    client: &reqwest::blocking::Client,
    url: &str,
    window: Rect,
    target: f64,
    threads: usize,
    place: &Place,
    visit: &(dyn Fn(&PointLayout, &[u8]) + Sync),
) -> Result<ReadStats, Failure> {
    use rayon::prelude::*;
    let (header, nodes, requests, window) = open(client, url, window, place)?;
    let level = choose_level(&header.info, &nodes, header.bounds, target);
    let chosen = select(&header.info, &nodes, window, level);
    let ranges = merge_ranges(&chosen, 256 * 1024, 24 * 1024 * 1024);
    let mut stats = ReadStats {
        level,
        nodes: chosen.len(),
        requests: requests + ranges.len(),
        ..Default::default()
    };
    let layout = header.layout;
    let items = header.laz.items().clone();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
        .map_err(|e| Failure::Other(e.to_string()))?;
    let results: Vec<Result<(u64, u64), Failure>> = pool.install(|| {
        ranges
            .par_iter()
            .map(|(start, len, group)| {
                fetch::within_laser_limit()?;
                let data = fetch::get_range(client, url, *start, *len).map_err(Failure::of_part)?;
                fetch::count_laser_bytes(data.len() as u64);
                if data.len() as u64 != *len {
                    return Err(Failure::Other("short range".into()));
                }
                let mut points = 0;
                for n in group {
                    let from = (n.offset - start) as usize;
                    let chunk = &data[from..from + n.size as usize];
                    let recs = decode(chunk, &items, n.count as usize, layout.record_len)
                        .map_err(Failure::Other)?;
                    visit(&layout, &recs);
                    points += n.count;
                }
                Ok((*len, points))
            })
            .collect()
    });
    for r in results {
        let (bytes, points) = r?;
        stats.bytes += bytes;
        stats.points += points;
    }
    Ok(stats)
}

/// Decode one node's LAZ chunk to raw point records.
fn decode(
    chunk: &[u8],
    items: &Vec<laz::LazItem>,
    count: usize,
    record_len: usize,
) -> Result<Vec<u8>, String> {
    let mut dec = LayeredPointRecordDecompressor::new(std::io::Cursor::new(chunk));
    dec.set_fields_from(items).map_err(|e| e.to_string())?;
    // Position, returns and class always come; of the rest only the intensity.
    dec.set_selection(DecompressionSelection(
        DecompressionSelection::Z
            | DecompressionSelection::CLASSIFICATION
            | DecompressionSelection::INTENSITY,
    ));
    let mut out = vec![0u8; count * record_len];
    dec.decompress_many(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 km cube like IGN's: root at level 0, four children, sixteen
    /// grandchildren, with the density of each level as in a real tile.
    fn tile() -> (Info, Vec<Node>) {
        let info = Info {
            center: [500.0, 500.0, 0.0],
            halfsize: 500.0,
            root_offset: 0,
            root_size: 0,
        };
        let mut nodes = Vec::new();
        let mut offset = 1000;
        for (level, count) in [(0, 50_000u64), (1, 125_000), (2, 350_000)] {
            let n = 1 << level;
            for y in 0..n {
                for x in 0..n {
                    nodes.push(Node {
                        level,
                        x,
                        y,
                        offset,
                        size: count * 6,
                        count,
                    });
                    offset += count * 6;
                }
            }
        }
        (info, nodes)
    }

    #[test]
    fn node_squares_tile_the_cube() {
        let (info, _) = tile();
        assert_eq!(node_rect(&info, 0, 0, 0), (0.0, 0.0, 1000.0, 1000.0));
        assert_eq!(node_rect(&info, 2, 3, 1), (750.0, 250.0, 1000.0, 500.0));
    }

    #[test]
    fn the_level_is_the_shallowest_dense_enough() {
        let (info, nodes) = tile();
        let all = (0.0, 0.0, 1000.0, 1000.0);
        // Cumulative densities: 0.05, 0.55, 6.15 points a square metre.
        assert_eq!(choose_level(&info, &nodes, all, 0.5), 1);
        assert_eq!(choose_level(&info, &nodes, all, 5.0), 2);
        // Asking for more than there is reads everything.
        assert_eq!(choose_level(&info, &nodes, all, 50.0), 2);
        // Points over half the tile (a coast) are as dense where they are.
        let half: Vec<Node> = nodes
            .iter()
            .filter(|n| node_rect(&info, n.level, n.x, n.y).0 < 500.0 || n.level < 2)
            .copied()
            .collect();
        assert_eq!(choose_level(&info, &half, all, 5.0), 2);
    }

    #[test]
    fn only_nodes_over_the_window_are_read() {
        let (info, nodes) = tile();
        // A window inside the south-west grandchild.
        let w = (10.0, 10.0, 200.0, 200.0);
        let sel = select(&info, &nodes, w, 2);
        assert_eq!(sel.len(), 3);
        assert!(sel.iter().all(|n| n.x == 0 && n.y == 0));
        // Down to level 1 only: the root and one child.
        assert_eq!(select(&info, &nodes, w, 1).len(), 2);
        // A window over the middle meets the four central grandchildren.
        let mid = (400.0, 400.0, 600.0, 600.0);
        let sel = select(&info, &nodes, mid, 2);
        assert_eq!(sel.iter().filter(|n| n.level == 2).count(), 4);
    }

    #[test]
    fn neighbours_in_the_file_share_a_request() {
        let n = |offset, size| Node {
            level: 3,
            x: 0,
            y: 0,
            offset,
            size,
            count: 1,
        };
        let nodes = [n(0, 100), n(100, 50), n(1000, 10), n(5000, 10)];
        let r = merge_ranges(&nodes, 1000, 1 << 20);
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].0, r[0].1, r[0].2.len()), (0, 1010, 3));
        assert_eq!((r[1].0, r[1].1), (5000, 10));
        // A size cap splits a run.
        assert_eq!(merge_ranges(&nodes, 1000, 200).len(), 3);
    }

    #[test]
    fn hierarchy_entries_parse() {
        let mut page = Vec::new();
        for (l, x, y, z, off, size, count) in [
            (0i32, 0i32, 0i32, 0i32, 500u64, 120i32, 40i32),
            (1, 1, 0, 0, 900, 64, -1),
            (1, 0, 1, 0, 0, 0, 0),
        ] {
            for v in [l, x, y, z] {
                page.extend_from_slice(&v.to_le_bytes());
            }
            page.extend_from_slice(&off.to_le_bytes());
            page.extend_from_slice(&size.to_le_bytes());
            page.extend_from_slice(&count.to_le_bytes());
        }
        let (nodes, pages) = parse_page(&page).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            (nodes[0].offset, nodes[0].size, nodes[0].count),
            (500, 120, 40)
        );
        assert_eq!(pages, vec![(1, 1, 0, 900, 64)]);
    }

    #[test]
    fn hierarchy_entries_out_of_range_are_refused() {
        let entry = |l: i32, off: u64, size: i32, count: i32| {
            let mut e = Vec::new();
            for v in [l, 0, 0, 0] {
                e.extend_from_slice(&v.to_le_bytes());
            }
            e.extend_from_slice(&off.to_le_bytes());
            e.extend_from_slice(&size.to_le_bytes());
            e.extend_from_slice(&count.to_le_bytes());
            e
        };
        assert!(parse_page(&entry(i32::MAX, 0, 10, 5)).is_err());
        assert!(parse_page(&entry(3, u64::MAX - 5, 10, 5)).is_err());
        assert!(parse_page(&entry(3, 0, 10, i32::MAX)).is_err());
        assert!(parse_page(&entry(3, 0, 10, 5)).is_ok());
        // Records too short for their format are refused before any is read.
        let short = PointLayout {
            format: 6,
            record_len: 0,
            scale: [1.0; 3],
            offset: [0.0; 3],
        };
        assert!(short.check().is_err());
        assert!(PointLayout {
            record_len: 30,
            ..short
        }
        .check()
        .is_ok());
        assert!(PointLayout {
            format: 11,
            record_len: 99,
            ..short
        }
        .check()
        .is_err());
    }
}
