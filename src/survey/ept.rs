//! Entwine point tiles (EPT) read over HTTP, in part.
//!
//! An EPT survey is one octree over the whole survey, in a cube given by its
//! `ept.json`. Every node is a small LAZ file of its own (`ept-data/D-X-Y-Z.laz`,
//! some ten or twenty thousand points), and the hierarchy lists every node's
//! point count in JSON pages (`ept-hierarchy/D-X-Y-Z.json`, a count of -1 marks
//! the root of a further page). Like a COPC file, each level holds a thinned
//! sample and the deeper levels add the rest.
//!
//! So an area is read as: `ept.json`, the hierarchy pages over the area, then
//! the nodes over the area down to the level that gives the density asked for.
//! A survey can span a state, so its shallow nodes cover tens of kilometres
//! with a few points over the area; nodes that sparse are skipped, which loses
//! a few per cent of the points and saves a dozen downloads.

use super::copc::{PointLayout, ReadStats};
use super::crs::Crs;
use super::fetch::{self, Failure};
use super::source::Rect;

/// The parts of `ept.json` a reader needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    /// The octree's cube: west, south, east, north (its height is not needed).
    pub cube: Rect,
}

/// One node with points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub level: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub count: u64,
}

impl Node {
    fn key(&self) -> String {
        format!("{}-{}-{}-{}", self.level, self.x, self.y, self.z)
    }
}

/// A node under this many points a square metre of ground is not read.
const SPARSE: f64 = 0.1;

pub fn parse_info(json: &serde_json::Value) -> Result<Info, String> {
    let b: Vec<f64> = json["bounds"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default();
    if b.len() != 6 {
        return Err("ept.json without bounds".into());
    }
    match json["dataType"].as_str() {
        Some("laszip") => {}
        other => return Err(format!("EPT data type {other:?}")),
    }
    Ok(Info {
        cube: (b[0], b[1], b[3], b[4]),
    })
}

/// The square a node covers: west, south, east, north.
pub fn node_rect(info: &Info, level: i32, x: i32, y: i32) -> Rect {
    let (x0, y0, x1, _) = info.cube;
    let size = (x1 - x0) / f64::from(1u32 << level.clamp(0, 30));
    (
        x0 + f64::from(x) * size,
        y0 + f64::from(y) * size,
        x0 + f64::from(x + 1) * size,
        y0 + f64::from(y + 1) * size,
    )
}

fn meets(a: Rect, b: Rect) -> bool {
    a.0 < b.2 && a.2 > b.0 && a.1 < b.3 && a.3 > b.1
}

/// A node's key: level, x, y, z.
pub type Key = (i32, i32, i32, i32);

/// One hierarchy page over the window: its nodes with points, and the keys of
/// the further pages it points to.
pub fn parse_page(info: &Info, page: &serde_json::Value, window: Rect) -> (Vec<Node>, Vec<Key>) {
    let mut nodes = Vec::new();
    let mut pages = Vec::new();
    let Some(map) = page.as_object() else {
        return (nodes, pages);
    };
    for (key, count) in map {
        let mut it = key.split('-').map(|v| v.parse::<i32>());
        let (Some(Ok(level)), Some(Ok(x)), Some(Ok(y)), Some(Ok(z))) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        // Past this no survey goes; levels are counted through one by one.
        if !(0..=super::copc::MAX_LEVEL).contains(&level) {
            continue;
        }
        if !meets(node_rect(info, level, x, y), window) {
            continue;
        }
        match count.as_i64() {
            Some(-1) => pages.push((level, x, y, z)),
            Some(n) if n > 0 => nodes.push(Node {
                level,
                x,
                y,
                z,
                count: n as u64,
            }),
            _ => {}
        }
    }
    (nodes, pages)
}

/// The shallowest level whose points, with all above it, give `target` points a
/// square metre of ground over the nodes found, else the deepest there is.
/// `ground` is the square metres of ground in one of the grid's.
pub fn choose_level(info: &Info, nodes: &[Node], ground: f64, target: f64) -> i32 {
    use std::collections::{HashMap, HashSet};
    let mut points: HashMap<i32, f64> = HashMap::new();
    let mut squares: HashMap<i32, HashSet<(i32, i32)>> = HashMap::new();
    for n in nodes {
        *points.entry(n.level).or_default() += n.count as f64;
        squares.entry(n.level).or_default().insert((n.x, n.y));
    }
    let deepest = nodes.iter().map(|n| n.level).max().unwrap_or(0);
    let mut density = 0.0;
    for level in 0..=deepest {
        let Some(sq) = squares.get(&level) else {
            continue;
        };
        let r = node_rect(info, level, 0, 0);
        let area = (r.2 - r.0) * (r.3 - r.1) * ground * sq.len() as f64;
        density += points[&level] / area;
        if density >= target {
            return level;
        }
    }
    deepest
}

/// The nodes down to `level`, leaving out the ones too sparse to matter.
pub fn select(info: &Info, nodes: &[Node], level: i32, ground: f64) -> Vec<Node> {
    nodes
        .iter()
        .filter(|n| {
            let r = node_rect(info, n.level, n.x, n.y);
            n.level <= level && n.count as f64 / ((r.2 - r.0) * (r.3 - r.1) * ground) >= SPARSE
        })
        .copied()
        .collect()
}

/// Read the points of `window` (Web Mercator metres) from the survey whose
/// `ept.json` is at `url`, at `target` points a square metre of ground, handing
/// each node's records to `visit`. Nodes are fetched on up to `threads` threads.
pub fn read(
    client: &reqwest::blocking::Client,
    url: &str,
    window: Rect,
    target: f64,
    threads: usize,
    visit: &(dyn Fn(&PointLayout, &[u8]) + Sync),
) -> Result<ReadStats, Failure> {
    use rayon::prelude::*;
    let base = url.trim_end_matches("ept.json").trim_end_matches('/');
    let body = fetch::get_bytes(client, url)?;
    fetch::count_laser_bytes(body.len() as u64);
    let json: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| Failure::Unusable(e.to_string()))?;
    let info = parse_info(&json).map_err(Failure::Unusable)?;
    // Web Mercator stretches the ground by 1 / cos(latitude) each way.
    let (lat, _) = Crs::WebMercator.inverse(0.0, (window.1 + window.3) / 2.0);
    let ground = lat.to_radians().cos().powi(2);

    let mut nodes = Vec::new();
    let mut pages = vec![(0, 0, 0, 0)];
    let mut requests = 1;
    while let Some((l, x, y, z)) = pages.pop() {
        let page = fetch::get_bytes(
            client,
            &format!("{base}/ept-hierarchy/{l}-{x}-{y}-{z}.json"),
        )
        .map_err(Failure::of_part)?;
        requests += 1;
        fetch::count_laser_bytes(page.len() as u64);
        let page: serde_json::Value =
            serde_json::from_slice(&page).map_err(|e| Failure::Other(e.to_string()))?;
        let (n, p) = parse_page(&info, &page, window);
        nodes.extend(n);
        pages.extend(p.into_iter().filter(|&k| k != (l, x, y, z)));
        if requests > 100 {
            return Err(Failure::Other("hierarchy too large".into()));
        }
    }
    let level = choose_level(&info, &nodes, ground, target);
    let chosen = select(&info, &nodes, level, ground);
    let mut stats = ReadStats {
        level,
        nodes: chosen.len(),
        requests: requests + chosen.len(),
        ..Default::default()
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
        .map_err(|e| Failure::Other(e.to_string()))?;
    let results: Vec<Result<(u64, u64), Failure>> = pool.install(|| {
        chosen
            .par_iter()
            .map(|n| {
                fetch::within_laser_limit()?;
                let data = fetch::get_bytes(client, &format!("{base}/ept-data/{}.laz", n.key()))
                    .map_err(Failure::of_part)?;
                fetch::count_laser_bytes(data.len() as u64);
                let (layout, recs) = decode(&data).map_err(Failure::Other)?;
                visit(&layout, &recs);
                Ok((data.len() as u64, (recs.len() / layout.record_len) as u64))
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

/// Decode a whole small LAZ (or LAS) file to its layout and raw point records.
pub fn decode(data: &[u8]) -> Result<(PointLayout, Vec<u8>), String> {
    if data.len() < 227 || &data[..4] != b"LASF" {
        return Err("not a LAS/LAZ file".into());
    }
    let u16_at = |o: usize| u16::from_le_bytes([data[o], data[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    let f64_at = |o: usize| f64::from_le_bytes(data[o..o + 8].try_into().unwrap());
    let header_size = usize::from(u16_at(94));
    let point_offset = u32_at(96) as usize;
    let n_vlrs = u32_at(100);
    let format = data[104] & 0x3F;
    let compressed = data[104] & 0x80 != 0;
    let record_len = usize::from(u16_at(105));
    let mut count = u64::from(u32_at(107));
    if count == 0 && data[25] >= 4 && data.len() >= 255 {
        count = u64::from_le_bytes(data[247..255].try_into().unwrap());
    }
    let layout = PointLayout {
        format,
        record_len,
        scale: [f64_at(131), f64_at(139), f64_at(147)],
        offset: [f64_at(155), f64_at(163), f64_at(171)],
    };
    layout.check()?;
    // A node holds some tens of thousands of points; the bound keeps a
    // header that says otherwise from asking for gigabytes.
    if count > 10_000_000 || count * record_len as u64 > 256 << 20 {
        return Err(format!(
            "unexpected point records ({record_len} bytes, {count})"
        ));
    }
    let mut laz = None;
    let mut pos = header_size;
    for _ in 0..n_vlrs {
        if pos + 54 > data.len() {
            return Err("records past the end".into());
        }
        let user = String::from_utf8_lossy(&data[pos + 2..pos + 18]);
        let id = u16_at(pos + 18);
        let len = usize::from(u16_at(pos + 20));
        let at = pos + 54;
        if at + len > data.len() {
            return Err("records past the end".into());
        }
        if user.trim_end_matches('\0') == "laszip encoded" && id == 22204 {
            laz = Some(laz::LazVlr::read_from(&data[at..at + len]).map_err(|e| e.to_string())?);
        }
        pos = at + len;
    }
    let mut out = vec![0u8; count as usize * record_len];
    match laz {
        Some(vlr) => {
            let mut src = std::io::Cursor::new(data);
            src.set_position(point_offset as u64);
            let mut dec = laz::LasZipDecompressor::new(src, vlr).map_err(|e| e.to_string())?;
            dec.decompress_many(&mut out).map_err(|e| e.to_string())?;
        }
        None if !compressed => {
            let end = point_offset + out.len();
            if end > data.len() {
                return Err("short LAS file".into());
            }
            out.copy_from_slice(&data[point_offset..end]);
        }
        None => return Err("LAZ without its record".into()),
    }
    Ok((layout, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> Info {
        Info {
            cube: (0.0, 0.0, 6400.0, 6400.0),
        }
    }

    #[test]
    fn info_parses() {
        let j: serde_json::Value = serde_json::from_str(
            r#"{"bounds":[-9365357,5166194,-32520,-9298647,5232904,34190],
                "dataType":"laszip","span":128}"#,
        )
        .unwrap();
        assert_eq!(
            parse_info(&j).unwrap().cube,
            (-9_365_357.0, 5_166_194.0, -9_298_647.0, 5_232_904.0)
        );
        let bin: serde_json::Value =
            serde_json::from_str(r#"{"bounds":[0,0,0,1,1,1],"dataType":"binary"}"#).unwrap();
        assert!(parse_info(&bin).is_err());
    }

    #[test]
    fn pages_keep_only_what_meets_the_window() {
        let page: serde_json::Value = serde_json::from_str(
            r#"{"0-0-0-0": 7600, "1-0-0-0": 6919, "1-1-1-0": 7106,
                "2-0-0-0": -1, "2-3-3-1": 400, "junk": 5, "2-0-1-0": 0}"#,
        )
        .unwrap();
        // A window in the south-west quarter's south-west quarter.
        let (mut nodes, pages) = parse_page(&info(), &page, (100.0, 100.0, 900.0, 900.0));
        nodes.sort_by_key(|n| n.level);
        assert_eq!(
            nodes
                .iter()
                .map(|n| (n.level, n.x, n.y))
                .collect::<Vec<_>>(),
            vec![(0, 0, 0), (1, 0, 0)]
        );
        assert_eq!(pages, vec![(2, 0, 0, 0)]);
        assert_eq!(nodes[1].key(), "1-0-0-0");
    }

    #[test]
    fn the_level_is_the_shallowest_dense_enough_and_sparse_nodes_are_skipped() {
        let i = info();
        // Root 6.4 km: 4000 points (1e-4 a m²); level 6 nodes 100 m: 20_000 (2 a
        // m²); level 7 nodes 50 m: 15_000 each (6 a m²).
        let mut nodes = vec![Node {
            level: 0,
            x: 0,
            y: 0,
            z: 0,
            count: 4000,
        }];
        for x in 0..2 {
            nodes.push(Node {
                level: 6,
                x,
                y: 0,
                z: 0,
                count: 20_000,
            });
        }
        for x in 0..4 {
            nodes.push(Node {
                level: 7,
                x,
                y: 0,
                z: 0,
                count: 15_000,
            });
        }
        assert_eq!(choose_level(&i, &nodes, 1.0, 1.5), 6);
        assert_eq!(choose_level(&i, &nodes, 1.0, 7.0), 7);
        assert_eq!(choose_level(&i, &nodes, 1.0, 50.0), 7);
        // At 45 degrees a Mercator square metre is half a square metre of ground.
        assert_eq!(choose_level(&i, &nodes, 0.5, 3.5), 6);
        let sel = select(&i, &nodes, 7, 1.0);
        assert_eq!(sel.len(), 6);
        assert!(sel.iter().all(|n| n.level > 0));
    }

    #[test]
    fn plain_las_decodes() {
        // A LAS 1.2 header with two format-1 points of 28 bytes.
        let mut d = vec![0u8; 227];
        d[..4].copy_from_slice(b"LASF");
        d[24] = 1;
        d[25] = 2;
        d[94..96].copy_from_slice(&227u16.to_le_bytes());
        d[96..100].copy_from_slice(&227u32.to_le_bytes());
        d[104] = 1;
        d[105..107].copy_from_slice(&28u16.to_le_bytes());
        d[107..111].copy_from_slice(&2u32.to_le_bytes());
        for (o, v) in [(131, 0.01f64), (139, 0.01), (147, 0.01), (155, 100.0)] {
            d[o..o + 8].copy_from_slice(&v.to_le_bytes());
        }
        d.extend(std::iter::repeat_n(7u8, 56));
        let (l, recs) = decode(&d).unwrap();
        assert_eq!((l.format, l.record_len, recs.len()), (1, 28, 56));
        assert_eq!(l.offset[0], 100.0);
        assert!(decode(&d[..100]).is_err());
    }
}
