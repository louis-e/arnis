//! Work units: a large One World selection cut into region-group pieces.
//!
//! The cut lines sit on a lattice of `512 * n` blocks anchored at block 0 of
//! the world's frame, not at the selection, so every piece owns whole region
//! files (no two pieces write one `r.X.Z.mca`) and two jobs over overlapping
//! selections cut the same lines. Each piece's lat/lon bbox snaps back to
//! exactly its rectangle, so a run given that bbox builds that piece.

use crate::args::UnitSize;
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::{LLBBox, LLShapes};
use crate::projection::{llbbox_for_rect, snap_bbox_to_chunks, WebMercatorProjection};

const REGION_BLOCKS: i32 = 512;

#[derive(Clone, Debug)]
pub struct WorkUnit {
    /// Position in the plan, which is also the build order.
    pub index: u32,
    /// Lattice cell, in groups of `size` regions.
    pub gx: i32,
    pub gz: i32,
    /// Side of the lattice cell, in regions.
    pub size: i32,
    pub rect: XZBBox,
    pub bbox: LLBBox,
    /// Region files the piece touches.
    pub regions: u64,
}

impl WorkUnit {
    pub fn chunks(&self) -> u64 {
        let w = (self.rect.max_x() - self.rect.min_x() + 1) as u64 / 16;
        let h = (self.rect.max_z() - self.rect.min_z() + 1) as u64 / 16;
        w * h
    }

    /// `rx,rz,s`: the piece's north-west region and its lattice size, which no other piece
    /// of any plan on this world shares unless it covers the same regions.
    pub fn region_key(&self) -> String {
        format!(
            "{},{},{}",
            self.rect.min_x().div_euclid(REGION_BLOCKS),
            self.rect.min_z().div_euclid(REGION_BLOCKS),
            self.size
        )
    }
}

/// Cuts `world` (already snapped to chunks) into pieces of at most `n` x `n`
/// regions, centre first.
#[cfg(test)]
pub fn plan_units(
    proj: &WebMercatorProjection,
    world: &XZBBox,
    n: i32,
) -> Result<Vec<WorkUnit>, String> {
    plan_cells(proj, world, n, false)
}

/// Edge strips thinner than this join their neighbour under `--snap-regions merge`.
const SLIVER_BLOCKS: i32 = REGION_BLOCKS / 4;

/// One axis of the lattice over `lo..=hi`: `(cell, first, last)` per cell, clipped.
fn segments(lo: i32, hi: i32, side: i32, merge: bool) -> Vec<(i32, i32, i32)> {
    let mut v: Vec<_> = (lo.div_euclid(side)..=hi.div_euclid(side))
        .map(|g| (g, (g * side).max(lo), (g * side + side - 1).min(hi)))
        .collect();
    let thin = |s: &(i32, i32, i32)| s.2 - s.1 + 1 < SLIVER_BLOCKS;
    if merge && v.len() > 1 && thin(&v[0]) {
        let first = v.remove(0);
        v[0].1 = first.1;
    }
    if merge && v.len() > 1 && thin(&v[v.len() - 1]) {
        let last = v.pop().unwrap_or_default();
        let k = v.len() - 1;
        v[k].2 = last.2;
    }
    v
}

fn plan_cells(
    proj: &WebMercatorProjection,
    world: &XZBBox,
    n: i32,
    merge: bool,
) -> Result<Vec<WorkUnit>, String> {
    if n < 1 {
        return Err("a work unit is at least one region".to_string());
    }
    let side = REGION_BLOCKS * n;
    let mut units = Vec::new();
    for (gz, z0, z1) in segments(world.min_z(), world.max_z(), side, merge) {
        for &(gx, x0, x1) in &segments(world.min_x(), world.max_x(), side, merge) {
            let rect = XZBBox::rect_from_min_max(x0, z0, x1, z1)?;
            units.push(unit(proj, rect, gx, gz, n)?);
        }
    }
    order_centre_out(world, &mut units);
    Ok(units)
}

/// The piece over `rect`, lattice cell `gx, gz` of `size` regions; numbered later.
fn unit(
    proj: &WebMercatorProjection,
    rect: XZBBox,
    gx: i32,
    gz: i32,
    size: i32,
) -> Result<WorkUnit, String> {
    let regions =
        |a: i32, b: i32| (b.div_euclid(REGION_BLOCKS) - a.div_euclid(REGION_BLOCKS) + 1) as u64;
    Ok(WorkUnit {
        index: 0,
        gx,
        gz,
        size,
        regions: regions(rect.min_x(), rect.max_x()) * regions(rect.min_z(), rect.max_z()),
        bbox: llbbox_for_rect(proj, &rect)?,
        rect,
    })
}

/// Numbers `units` from the centre of `world` out, in doubled block units to stay integral;
/// ties go row-major.
fn order_centre_out(world: &XZBBox, units: &mut [WorkUnit]) {
    let (cx, cz) = (
        i64::from(world.min_x()) + i64::from(world.max_x()),
        i64::from(world.min_z()) + i64::from(world.max_z()),
    );
    units.sort_by_key(|u| {
        let ux = i64::from(u.rect.min_x()) + i64::from(u.rect.max_x()) - cx;
        let uz = i64::from(u.rect.min_z()) + i64::from(u.rect.max_z()) - cz;
        (ux * ux + uz * uz, u.gz, u.gx)
    });
    for (i, u) in units.iter_mut().enumerate() {
        u.index = i as u32;
    }
}

/// `--plan-mask`: the polygon a job keeps its pieces in, as an area shape over the job's
/// rectangle, so a piece is kept when some column of it is inside, as a run would build it.
pub struct Mask {
    shape: XZBBox,
    /// Of the file, so two masks over one rectangle keep apart job folders.
    hash: u64,
}

impl Mask {
    /// A GeoJSON (Multi)Polygon, Feature or FeatureCollection, or an Osmosis `.poly` file.
    pub fn read(path: &std::path::Path) -> Result<(LLShapes, u64), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("--plan-mask {}: {e}", path.display()))?;
        // A .poly file is one polygon whose rings go even-odd, holes or not.
        let polygons = LLShapes::from_geojson(&text).unwrap_or_else(|_| {
            let rings: Vec<Vec<(f64, f64)>> = poly_rings(&text)
                .into_iter()
                .map(|r| r.into_iter().map(|(lon, lat)| (lat, lon)).collect())
                .collect();
            if rings.is_empty() {
                Vec::new()
            } else {
                vec![rings]
            }
        });
        if polygons.is_empty() {
            return Err(format!(
                "--plan-mask {}: no polygon found (GeoJSON or .poly).",
                path.display()
            ));
        }
        let shapes =
            LLShapes::new(polygons).map_err(|e| format!("--plan-mask {}: {e}", path.display()))?;
        // FNV-1a: stable across builds, unlike the std hasher.
        let hash = {
            use std::hash::Hasher;
            let mut h = fnv::FnvHasher::default();
            h.write(text.as_bytes());
            h.finish()
        };
        Ok((shapes, hash))
    }

    /// The file's polygons over `rect`, in `proj`'s blocks. Fails when they hold none of it.
    pub fn load(
        path: &std::path::Path,
        proj: &WebMercatorProjection,
        rect: &XZBBox,
    ) -> Result<Self, String> {
        let (shapes, hash) = Self::read(path)?;
        let polygons = shapes
            .polygons()
            .iter()
            .map(|rings| {
                rings
                    .iter()
                    .map(|ring| {
                        ring.iter()
                            .map(|p| (proj.x_for_lon(p.lng()), proj.z_for_lat(p.lat())))
                            .collect()
                    })
                    .collect()
            })
            .collect();
        let shape = rect
            .clone()
            .with_outline(polygons)
            .map_err(|e| format!("--plan-mask: the polygon holds no piece of this job ({e})"))?;
        Ok(Self { shape, hash })
    }

    /// Whether some column of `r` is in the polygon.
    pub fn touches(&self, r: &XZBBox) -> bool {
        self.shape
            .touches_rect(r.min_x(), r.min_z(), r.max_x(), r.max_z())
    }

    /// Whether every column of `r` is in the polygon.
    fn covers(&self, r: &XZBBox) -> bool {
        self.shape
            .covers_rect(r.min_x(), r.min_z(), r.max_x(), r.max_z())
    }
}

/// Rings of an Osmosis `.poly` file: a name line, then sections of `lon lat` lines each
/// closed by `END` (a `!` section is a hole, which even-odd handles alike), then `END`.
fn poly_rings(text: &str) -> Vec<Vec<(f64, f64)>> {
    let mut rings = Vec::new();
    let mut ring = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace().map(str::parse::<f64>);
        match (parts.next(), parts.next()) {
            (Some(Ok(lon)), Some(Ok(lat))) => ring.push((lon, lat)),
            _ if line.trim() == "END" && ring.len() > 2 => rings.push(std::mem::take(&mut ring)),
            _ => ring.clear(),
        }
    }
    rings
}

/// Sizes `--unit-regions auto` mixes, largest first.
const AUTO_SIZES: [i32; 3] = [8, 4, 2];

/// `--unit-regions auto`: a quadtree on the `max`-region lattice anchored at block 0. A
/// lattice cell wholly inside `world` is one piece; one that is not splits into four, down
/// to `min`, where what is left is clipped as a fixed size is.
fn auto_cells(
    proj: &WebMercatorProjection,
    world: &XZBBox,
    max: i32,
    min: i32,
    mask: Option<&Mask>,
) -> Result<Vec<WorkUnit>, String> {
    fn split(
        proj: &WebMercatorProjection,
        world: &XZBBox,
        (gx, gz, s): (i32, i32, i32),
        min: i32,
        mask: Option<&Mask>,
        out: &mut Vec<WorkUnit>,
    ) -> Result<(), String> {
        let side = REGION_BLOCKS * s;
        let cell = [
            gx * side,
            gz * side,
            gx * side + side - 1,
            gz * side + side - 1,
        ];
        let clipped = [
            cell[0].max(world.min_x()),
            cell[1].max(world.min_z()),
            cell[2].min(world.max_x()),
            cell[3].min(world.max_z()),
        ];
        if clipped[0] > clipped[2] || clipped[1] > clipped[3] {
            return Ok(());
        }
        let rect = XZBBox::rect_from_min_max(clipped[0], clipped[1], clipped[2], clipped[3])?;
        // A cell the mask misses is dropped; one it only partly holds splits.
        let piece = unit(proj, rect, gx, gz, s)?;
        if mask.is_some_and(|m| !m.touches(&piece.rect)) {
            return Ok(());
        }
        if (clipped == cell && mask.is_none_or(|m| m.covers(&piece.rect))) || s <= min {
            out.push(piece);
            return Ok(());
        }
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            split(
                proj,
                world,
                (gx * 2 + dx, gz * 2 + dz, s / 2),
                min,
                mask,
                out,
            )?;
        }
        Ok(())
    }
    let side = REGION_BLOCKS * max;
    let mut units = Vec::new();
    for gz in world.min_z().div_euclid(side)..=world.max_z().div_euclid(side) {
        for gx in world.min_x().div_euclid(side)..=world.max_x().div_euclid(side) {
            split(proj, world, (gx, gz, max), min, mask, &mut units)?;
        }
    }
    order_centre_out(world, &mut units);
    Ok(units)
}

/// The plan indices of `units` (numbered `0..len`) in the order `--piece-order` builds them.
pub fn build_order(
    units: &[WorkUnit],
    order: Option<&crate::args::PieceOrder>,
) -> Result<Vec<usize>, String> {
    use crate::args::PieceOrder;
    let mut ids: Vec<usize> = (0..units.len()).collect();
    let r = |i: usize| &units[i].rect;
    match order {
        None | Some(PieceOrder::Centre) => {}
        Some(PieceOrder::West) => ids.sort_by_key(|&i| (r(i).min_x(), i)),
        Some(PieceOrder::East) => ids.sort_by_key(|&i| (-r(i).max_x(), i)),
        Some(PieceOrder::North) => ids.sort_by_key(|&i| (r(i).min_z(), i)),
        Some(PieceOrder::South) => ids.sort_by_key(|&i| (-r(i).max_z(), i)),
        Some(PieceOrder::List(first)) => {
            if let Some(bad) = first.iter().find(|&&n| n as usize > units.len()) {
                return Err(format!(
                    "--piece-order {bad}: this job has {} pieces.",
                    units.len()
                ));
            }
            let mut listed: Vec<usize> = Vec::new();
            for &n in first {
                if !listed.contains(&(n as usize - 1)) {
                    listed.push(n as usize - 1);
                }
            }
            ids.retain(|i| !listed.contains(i));
            listed.extend(ids);
            ids = listed;
        }
    }
    Ok(ids)
}

/// A job's pieces and the block rectangle they tile, from the selection and the job flags.
pub struct Plan {
    /// The selection snapped to chunks, as a run without the job flags would build it.
    pub requested: XZBBox,
    /// What the job builds: `requested` after `--snap-regions` and `--margin-regions`.
    pub rect: XZBBox,
    pub units: Vec<WorkUnit>,
    /// The job's folder name: one plan, one folder.
    pub name: String,
}

/// The plan a job runs and `--plan-units` prints: both call this, so they agree.
pub fn plan(
    proj: &WebMercatorProjection,
    selection: &LLBBox,
    args: &crate::args::Args,
    size: UnitSize,
) -> Result<Plan, String> {
    use crate::args::SnapRegions;
    let units = &args.units;
    let (requested, _) = snap_bbox_to_chunks(proj, selection)?;
    // Mixed sizes snap to the smallest, so every leaf of a covered job is whole.
    let n = match size {
        UnitSize::N(n) => n,
        UnitSize::Auto => AUTO_SIZES[2],
    };
    let side = REGION_BLOCKS * n;
    let axis = |lo: i32, hi: i32| match units.snap_regions {
        Some(SnapRegions::Cover) => (
            lo.div_euclid(side) * side,
            (hi.div_euclid(side) + 1) * side - 1,
        ),
        Some(SnapRegions::Fit) => {
            let (a, b) = ((lo + side - 1).div_euclid(side), (hi + 1).div_euclid(side));
            // Not one whole piece inside: the one under the centre.
            let (a, b) = if b > a {
                (a, b)
            } else {
                let c = (lo + hi).div_euclid(2).div_euclid(side);
                (c, c + 1)
            };
            (a * side, b * side - 1)
        }
        _ => (lo, hi),
    };
    let m = units.margin_regions * REGION_BLOCKS;
    let (x0, x1) = axis(requested.min_x(), requested.max_x());
    let (z0, z1) = axis(requested.min_z(), requested.max_z());
    let rect = XZBBox::rect_from_min_max(x0 - m, z0 - m, x1 + m, z1 + m)?;
    let merge = units.snap_regions == Some(SnapRegions::Merge);
    let mask = units
        .plan_mask
        .as_deref()
        .map(|path| Mask::load(path, proj, &rect))
        .transpose()?;
    // The largest auto size follows the memory, so it names the job: a resume on a machine
    // that picks another size opens another folder instead of misreading this one's pieces.
    let auto_max = (size == UnitSize::Auto).then(|| crate::scale::budget::auto_max_regions(args));
    let mut cut = match (size, auto_max) {
        (UnitSize::N(n), _) => plan_cells(proj, &rect, n, merge)?,
        (UnitSize::Auto, max) => {
            let max = max.unwrap_or(AUTO_SIZES[0]);
            // Whole `max` pieces clipped at the edges: smaller edge pieces cost more than they
            // save (B3, 256 km2: 136 mixed pieces 93 s, 16 of 8 x 8 65 s). An outline still
            // splits, larger leaves until the map ids go round.
            let mut cut = Vec::new();
            let leaves = AUTO_SIZES.into_iter().rev();
            for min in leaves.filter(|&s| s <= max && (mask.is_some() || s == max)) {
                cut = auto_cells(proj, &rect, max, min, mask.as_ref())?;
                if cut.len() <= crate::scale::MAX_PIECES {
                    break;
                }
            }
            cut
        }
    };
    let mut name = format!(
        "{}_{}_{}_{}_n{size}",
        rect.min_x(),
        rect.min_z(),
        rect.max_x(),
        rect.max_z()
    );
    if let Some(max) = auto_max {
        // `w`: whole pieces, not the mixed cut of 3.5.0, whose jobs keep their own folder.
        name.push_str(&format!("{max}{}", if mask.is_none() { "w" } else { "" }));
    }
    // Merged edges cut other pieces than a plain job over the same rectangle.
    if merge {
        name.push_str("_merge");
    }
    if let Some(mask) = &mask {
        cut.retain(|u| mask.touches(&u.rect));
        if cut.is_empty() {
            return Err("--plan-mask: the polygon holds no piece of this job.".to_string());
        }
        order_centre_out(&rect, &mut cut);
        name.push_str(&format!("_m{:016x}", mask.hash));
    }
    Ok(Plan {
        requested,
        rect,
        units: cut,
        name,
    })
}

/// Whole lattice cells of `n` regions around `rect`, about one `rect` out, whose ground is
/// stretched at most `max_stretch` (blocks per metre are fixed at the origin's latitude):
/// where a One World can grow next. At most `cap` cells; none when one ring would pass it.
pub fn room(
    proj: &WebMercatorProjection,
    rect: &XZBBox,
    n: i32,
    max_stretch: f64,
    cap: usize,
) -> Vec<LLBBox> {
    let side = REGION_BLOCKS * n.max(1);
    let (gx0, gx1) = (rect.min_x().div_euclid(side), rect.max_x().div_euclid(side));
    let (gz0, gz1) = (rect.min_z().div_euclid(side), rect.max_z().div_euclid(side));
    let (w, h) = (i64::from(gx1 - gx0 + 1), i64::from(gz1 - gz0 + 1));
    let count = |k: i64| ((w + 2 * k) * (h + 2 * k) - w * h) as usize;
    let mut k = w.max(h);
    while k > 0 && count(k) > cap {
        k -= 1;
    }
    let k = k as i32;
    let cos0 = proj.origin_lat.to_radians().cos();
    let stretch = |lat: f64| (cos0 / lat.to_radians().cos() - 1.0).abs();
    let mut out = Vec::new();
    for gz in gz0 - k..=gz1 + k {
        for gx in gx0 - k..=gx1 + k {
            if (gx0..=gx1).contains(&gx) && (gz0..=gz1).contains(&gz) {
                continue;
            }
            let Ok(cell) = XZBBox::rect_from_min_max(
                gx * side,
                gz * side,
                gx * side + side - 1,
                gz * side + side - 1,
            ) else {
                continue;
            };
            let Ok(bbox) = llbbox_for_rect(proj, &cell) else {
                continue;
            };
            if stretch(bbox.min().lat()).max(stretch(bbox.max().lat())) <= max_stretch {
                out.push(bbox);
            }
        }
    }
    out
}

/// `--plan-units N`: prints the plan as one JSON line and touches nothing.
pub fn print_plan(
    world_dir: &std::path::Path,
    requested: &LLBBox,
    args: &crate::args::Args,
    size: UnitSize,
) -> Result<(), String> {
    let plan = plan_line(world_dir, requested, args, size)?;
    for w in plan["warnings"].as_array().into_iter().flatten() {
        eprintln!("Warning: {}", w.as_str().unwrap_or_default());
    }
    println!("{plan}");
    Ok(())
}

/// The plan line `--plan-units` prints, which the window draws. A world that does not exist
/// yet is planned in the frame its first run would give it (centred on the request).
pub fn plan_line(
    world_dir: &std::path::Path,
    requested: &LLBBox,
    args: &crate::args::Args,
    size: UnitSize,
) -> Result<serde_json::Value, String> {
    let proj = crate::one_world::frame_for(world_dir, requested, args)?;
    let Plan {
        requested: asked,
        rect: world,
        units,
        name,
    } = plan(&proj, requested, args, size)?;
    let mut existing = 0;
    // The runner's own decision: Auto below the break-even is one plain run, so one worker.
    let single =
        crate::scale::budget::builds_single(args, requested.area_km2() * args.scale * args.scale);
    let workers = if single {
        1
    } else {
        crate::scale::budget::sizing(
            args,
            units.len(),
            units.iter().map(|u| u.regions).max().unwrap_or(1),
        )
        .workers
    };
    let (job_estimate, warnings) = job_estimate(&units, args, workers, world_dir);
    let skip = crate::scale::skipped_cells(world_dir, &name, args);
    let units: Vec<_> = units
        .iter()
        .map(|u| {
            let have = crate::one_world::existing_chunks(world_dir, &u.rect);
            existing += have;
            let e = Estimate::of(&[u], args.scale, 1);
            serde_json::json!({
                "index": u.index,
                // `key` is the lattice cell in units of the piece's own size, so pieces of
                // different sizes in an auto plan can share one; `region_key` ("rx,rz,size")
                // is the piece's unique id.
                "key": format!("{},{}", u.gx, u.gz),
                "size": u.size,
                "region_key": u.region_key(),
                "rect": [u.rect.min_x(), u.rect.min_z(), u.rect.max_x(), u.rect.max_z()],
                "bbox": u.bbox.arg(),
                "chunks": u.chunks(),
                "regions": u.regions,
                "existing_chunks": have,
                // Built by an earlier run of this same job, which a run now resumes.
                "done": crate::scale::piece_done(world_dir, &name, u),
                // Left out of the job (`--skip-cells`, or kept from an earlier run of it).
                "skip": skip.contains(&(u.index as usize)),
                // Wholly outside the selection: a piece of the margin ring.
                "ring": u.rect.max_x() < asked.min_x()
                    || asked.max_x() < u.rect.min_x()
                    || u.rect.max_z() < asked.min_z()
                    || asked.max_z() < u.rect.min_z(),
                "estimate": {"disk_mb": e.disk_mb, "ram_mb": e.ram_mb, "time_min": e.time_min},
            })
        })
        .collect();
    let mut plan = serde_json::json!({
        "v": 1,
        "type": "plan",
        "unit_regions": match size {
            UnitSize::N(n) => serde_json::json!(n),
            UnitSize::Auto => serde_json::json!("auto"),
        },
        "rect": [world.min_x(), world.min_z(), world.max_x(), world.max_z()],
        "requested": [asked.min_x(), asked.min_z(), asked.max_x(), asked.max_z()],
        // True when the job runs as one plain run; `units` still draws the cells it covers.
        "single": single,
        "pieces": if single { 1 } else { units.len() },
        "units": units,
        "existing_chunks": existing,
        "estimate": job_estimate,
    });
    if !warnings.is_empty() {
        plan["warnings"] = serde_json::json!(warnings);
    }
    Ok(plan)
}

/// On-disk KB per chunk, [low, high]. A chunk takes at least one 4,096-byte sector. Measured:
/// 4.0 on rural preview worlds (Grindelwald, Danube), 4.3 on dense Bucharest, 4.1 on a One
/// World; the T1 smokes (work/c35/t1-smoke) wrote 4.0 (rural, 16 regions), 4.06 (city,
/// 64) and 4.14 (parity area, 4). The top leaves room above the densest.
const DISK_KB_PER_CHUNK: [f64; 2] = [4.0, 4.6];
/// Previews and map items beside the region files, KB per region, from the same smokes:
/// 84 (rural), 220 (city) and 249 (parity area, many signage maps).
const EXTRA_KB_PER_REGION: [f64; 2] = [84.0, 249.0];
/// Wall seconds per 1,000 chunks for one worker. B3 (work/bench22): Auto on 256 km² of
/// Bucharest, 16 pieces, 4 workers x 5 threads on 24 cores at scale 1, caches warm, built
/// 1,000,000 chunks in 52.8 s (median of 3): 0.21; the range brackets it, so its middle
/// lands within 10%. Smaller and rural pieces ran slower per chunk in the T1 smokes.
const WORKER_S_PER_KCHUNK: [f64; 2] = [0.18, 0.28];

/// Ranges for some pieces built by `workers` at once.
#[derive(Debug, PartialEq)]
struct Estimate {
    disk_mb: [f64; 2],
    /// Per worker: the largest piece's, as the coordinator plans for.
    ram_mb: [f64; 2],
    time_min: [f64; 2],
}

impl Estimate {
    fn of(units: &[&WorkUnit], scale: f64, workers: usize) -> Self {
        let chunks: u64 = units.iter().map(|u| u.chunks()).sum();
        let regions: u64 = units.iter().map(|u| u.regions).sum();
        let ram = |regions: Option<u64>| {
            regions.map_or(0.0, |r| {
                crate::scale::budget::estimate_piece_mb(r, scale) as f64
            })
        };
        let at = |k: usize| {
            (
                (chunks as f64 * DISK_KB_PER_CHUNK[k] + regions as f64 * EXTRA_KB_PER_REGION[k])
                    / 1024.0,
                chunks as f64 / 1000.0 * WORKER_S_PER_KCHUNK[k] / workers.max(1) as f64 / 60.0,
            )
        };
        let round = |v: f64| (v * 10.0).round() / 10.0;
        let ((d0, t0), (d1, t1)) = (at(0), at(1));
        Self {
            disk_mb: [round(d0), round(d1)],
            ram_mb: [
                ram(units.iter().map(|u| u.regions).min()),
                ram(units.iter().map(|u| u.regions).max()),
            ],
            time_min: [round(t0), round(t1)],
        }
    }
}

/// The plan line's `estimate` for the whole job, and what it warns about.
fn job_estimate(
    units: &[WorkUnit],
    args: &crate::args::Args,
    workers: usize,
    world_dir: &std::path::Path,
) -> (serde_json::Value, Vec<String>) {
    let all: Vec<&WorkUnit> = units.iter().collect();
    let e = Estimate::of(&all, args.scale, workers);
    let mut basis = format!(
        "disk: chunks x {:?} KB plus {:?} KB per region for previews and map items, measured \
         on 3.5.0 worlds; ram_mb: per worker, scale::budget's piece estimate; time_min: \
         {workers} worker(s) at {:?} s per 1,000 chunks for one worker, measured with warm \
         caches on 24 cores at scale 1, so downloads add to it",
        DISK_KB_PER_CHUNK, EXTRA_KB_PER_REGION, WORKER_S_PER_KCHUNK
    );
    if args.dh_lod {
        basis.push_str("; the Distant Horizons LOD is not counted");
    }
    let mut out = serde_json::json!({
        "disk_mb": e.disk_mb, "ram_mb": e.ram_mb, "time_min": e.time_min, "workers": workers,
    });
    basis.push_str("; downloads not estimated (Overpass and elevation sizes are not known)");
    let mut warnings = Vec::new();
    // The disk holding the world, or its nearest existing parent.
    let free = world_dir
        .ancestors()
        .find(|p| p.exists())
        .and_then(|p| fs2::available_space(p).ok());
    if let Some(free) = free {
        let free_mb = (free / 1_048_576) as f64;
        out["free_disk_mb"] = serde_json::json!(free_mb);
        if e.disk_mb[1] > free_mb {
            warnings.push(format!(
                "this job may need up to {:.0} MB but the disk has {free_mb:.0} MB free.",
                e.disk_mb[1]
            ));
        }
    }
    out["basis"] = serde_json::json!(basis);
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_rings_the_job_within_the_stretch_band() {
        let proj = WebMercatorProjection::new(44.43, 26.1, 1.0);
        let one = XZBBox::rect_from_min_max(0, 0, 511, 511).unwrap();
        assert_eq!(room(&proj, &one, 1, 0.05, 400).len(), 8);
        assert!(room(&proj, &one, 1, 0.05, 7).is_empty());
        // 1000 km north of the origin is past 5%: nothing there.
        let far = XZBBox::rect_from_min_max(0, -1_400_000, 511, -1_399_489).unwrap();
        assert!(room(&proj, &far, 1, 0.05, 400).is_empty());
    }

    #[test]
    fn estimates_follow_the_measured_rates() {
        let (proj, world) = bucharest();
        let units = plan_units(&proj, &world, 1).unwrap();
        let one: Vec<&WorkUnit> = units.iter().take(1).collect();
        let (c, r) = (one[0].chunks() as f64, one[0].regions as f64);
        let e = Estimate::of(&one, 1.0, 1);
        let mb = |kb_chunk: f64, kb_region: f64| {
            ((c * kb_chunk + r * kb_region) / 1024.0 * 10.0).round() / 10.0
        };
        assert_eq!(e.disk_mb, [mb(4.0, 84.0), mb(4.6, 249.0)]);
        let ram = crate::scale::budget::estimate_piece_mb(one[0].regions, 1.0) as f64;
        assert_eq!(e.ram_mb, [ram, ram]);
        let min = |s: f64| (c / 1000.0 * s / 60.0 * 10.0).round() / 10.0;
        assert_eq!(e.time_min, [min(0.18), min(0.28)]);
        // Workers split the time, not the disk; RAM is per worker.
        let all: Vec<&WorkUnit> = units.iter().collect();
        let (a, b) = (Estimate::of(&all, 1.0, 1), Estimate::of(&all, 1.0, 4));
        assert_eq!(a.disk_mb, b.disk_mb);
        assert!((a.time_min[1] / 4.0 - b.time_min[1]).abs() <= 0.1);
        assert!(a.ram_mb[0] <= a.ram_mb[1]);
    }

    #[test]
    fn time_estimate_tracks_measured_s16() {
        // B3: tuned Auto on S16 (256 km², 1,000,000 chunks, 4 workers) took 52.8 s
        // (median of 3, work/bench22/results-b3.jsonl); the window shows the range's middle.
        let (lo, hi) = (WORKER_S_PER_KCHUNK[0], WORKER_S_PER_KCHUNK[1]);
        let shown_s = 1_000.0 * (lo + hi) / 2.0 / 4.0;
        assert!((shown_s / 52.8 - 1.0).abs() <= 0.25, "{shown_s} s");
    }

    fn bucharest() -> (WebMercatorProjection, XZBBox) {
        let proj = WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        (proj, snap_bbox_to_chunks(&proj, &req).unwrap().0)
    }

    fn r(b: &XZBBox) -> [i32; 4] {
        [b.min_x(), b.min_z(), b.max_x(), b.max_z()]
    }

    #[test]
    fn plan_units_cover_without_overlap() {
        let (proj, world) = bucharest();
        for n in [1, 2, 4] {
            let units = plan_units(&proj, &world, n).unwrap();
            assert!(units.len() > 1, "n={n}");
            let area: u64 = units.iter().map(WorkUnit::chunks).sum();
            let (w, h) = (
                world.max_x() - world.min_x() + 1,
                world.max_z() - world.min_z() + 1,
            );
            assert_eq!(
                area,
                (w as u64 / 16) * (h as u64 / 16),
                "n={n}: gap or overlap"
            );
            for (i, a) in units.iter().enumerate() {
                let [x0, z0, x1, z1] = r(&a.rect);
                assert!(x0 >= world.min_x() && x1 <= world.max_x());
                assert!(z0 >= world.min_z() && z1 <= world.max_z());
                for b in &units[i + 1..] {
                    let [bx0, bz0, bx1, bz1] = r(&b.rect);
                    let apart = x1 < bx0 || bx1 < x0 || z1 < bz0 || bz1 < z0;
                    assert!(apart, "n={n}: pieces {} and {} overlap", a.index, b.index);
                }
                // Whole region files: no two pieces share an r.X.Z.mca.
                assert_eq!(x0.div_euclid(512 * n), x1.div_euclid(512 * n));
                assert_eq!(z0.div_euclid(512 * n), z1.div_euclid(512 * n));
            }
        }
    }

    /// Doc 18's example: a new world centred on the request, 2x2-region pieces.
    fn doc18() -> (WebMercatorProjection, LLBBox) {
        let req = LLBBox::new(44.425, 26.08, 44.445, 26.13).unwrap();
        (WebMercatorProjection::new(44.435, 26.105, 1.0), req)
    }

    fn args_with(units: crate::args::UnitArgs) -> crate::args::Args {
        let mut args = <crate::args::Args as clap::Parser>::try_parse_from(["arnis"]).unwrap();
        args.units = units;
        args
    }

    #[test]
    fn auto_sizes_fill_the_job_with_the_largest_whole_pieces() {
        use crate::args::{SnapRegions, UnitArgs};
        let (proj, req) = doc18();
        let mut args = args_with(UnitArgs {
            snap_regions: Some(SnapRegions::Cover),
            ..Default::default()
        });
        // Room for two 8 x 8 pieces at scale 1 (976 MB each).
        args.ram_budget_mb = Some(4000);
        let p = plan(&proj, &req, &args, UnitSize::Auto).unwrap();
        // Covered to the 2-region lattice: 8 x 8 regions around block 0.
        assert_eq!(r(&p.rect), [-2048, -2048, 2047, 2047]);
        assert!(p.name.ends_with("_nauto8w"));
        // The 8-lattice lines at x 0 and z 0 cut it into four clipped 8 x 8 pieces.
        assert_eq!(sizes(&p.units), [(2048, 2048); 4]);
        assert!(p.units.iter().all(|u| u.size == 8));
        // A wide job: 8 x 8 pieces, clipped at its edges, never split smaller.
        let wide = LLBBox::new(44.38, 26.00, 44.49, 26.21).unwrap();
        let p = plan(&proj, &wide, &args, UnitSize::Auto).unwrap();
        assert!(p.units.iter().all(|u| u.size == 8), "{:?}", sizes(&p.units));
        for u in &p.units {
            let [x0, z0, x1, z1] = r(&u.rect);
            assert_eq!(x0.div_euclid(4096), x1.div_euclid(4096));
            assert_eq!(z0.div_euclid(4096), z1.div_euclid(4096));
        }
        let area: u64 = p.units.iter().map(WorkUnit::chunks).sum();
        let [x0, z0, x1, z1] = r(&p.rect);
        assert_eq!(area, ((x1 - x0 + 1) / 16 * ((z1 - z0 + 1) / 16)) as u64);
        // Less memory: 4 x 4 pieces, and another job folder.
        let eights = p.name.clone();
        args.ram_budget_mb = Some(1500);
        let p = plan(&proj, &wide, &args, UnitSize::Auto).unwrap();
        assert!(p.units.iter().all(|u| u.size == 4));
        assert!(p.name.ends_with("_nauto4w") && eights.ends_with("_nauto8w"));
        // Without snapping, the edges are clipped: four pieces either side of block 0.
        args.units.snap_regions = None;
        args.ram_budget_mb = Some(4000);
        let p = plan(&proj, &req, &args, UnitSize::Auto).unwrap();
        assert_eq!(p.units.len(), 4);
        assert_eq!(r(&p.rect), [-2000, -1120, 1999, 1119]);
    }

    fn sizes(units: &[WorkUnit]) -> Vec<(i32, i32)> {
        let mut v: Vec<_> = units
            .iter()
            .map(|u| {
                let [x0, z0, x1, z1] = r(&u.rect);
                (x1 - x0 + 1, z1 - z0 + 1)
            })
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn without_the_job_flags_the_plan_is_todays() {
        let (proj, req) = doc18();
        let p = plan(&proj, &req, &args_with(Default::default()), UnitSize::N(2)).unwrap();
        let world = snap_bbox_to_chunks(&proj, &req).unwrap().0;
        assert_eq!(r(&p.rect), r(&world));
        assert_eq!(r(&p.rect), [-2000, -1120, 1999, 1119]);
        let today = plan_units(&proj, &world, 2).unwrap();
        let key = |u: &[WorkUnit]| u.iter().map(|u| (u.index, r(&u.rect))).collect::<Vec<_>>();
        assert_eq!(key(&p.units), key(&today));
        assert_eq!(p.name, "-2000_-1120_1999_1119_n2");
        // The 16 pieces doc 18 found, slivers and all.
        let mut want = [(1024, 1024); 4].to_vec();
        want.extend([(976, 1024); 4]);
        want.extend([(1024, 96); 4]);
        want.extend([(976, 96); 4]);
        want.sort_unstable();
        assert_eq!(sizes(&p.units), want);
    }

    #[test]
    fn snapped_jobs_are_whole_pieces() {
        use crate::args::{SnapRegions, UnitArgs};
        let (proj, req) = doc18();
        let with = |snap, margin| {
            args_with(UnitArgs {
                snap_regions: Some(snap),
                margin_regions: margin,
                ..Default::default()
            })
        };
        // Cover: every piece is exactly n x n regions.
        for (n, z, pieces) in [(1, 1536, 48), (2, 2048, 16), (4, 2048, 4)] {
            let p = plan(&proj, &req, &with(SnapRegions::Cover, 0), UnitSize::N(n)).unwrap();
            let side = 512 * n;
            assert!(sizes(&p.units).iter().all(|&s| s == (side, side)), "n={n}");
            assert_eq!(r(&p.rect), [-2048, -z, 2047, z - 1], "n={n}");
            assert_eq!(p.units.len(), pieces, "n={n}");
        }
        // Fit: whole pieces inside the selection.
        let p = plan(&proj, &req, &with(SnapRegions::Fit, 0), UnitSize::N(2)).unwrap();
        assert_eq!(r(&p.rect), [-1024, -1024, 1023, 1023]);
        assert_eq!(sizes(&p.units), [(1024, 1024); 4]);
        // Nothing whole fits: the piece under the centre.
        let p = plan(&proj, &req, &with(SnapRegions::Fit, 0), UnitSize::N(4)).unwrap();
        assert_eq!(r(&p.rect), [-2048, -2048, -1, -1]);
        // A margin of one region on every side.
        let p = plan(&proj, &req, &with(SnapRegions::Cover, 1), UnitSize::N(2)).unwrap();
        assert_eq!(r(&p.rect), [-2560, -2560, 2559, 2559]);
        assert_eq!(p.name, "-2560_-2560_2559_2559_n2");
    }

    /// Doc 21 A2: whatever the snap, margin and size, a plan tiles its rectangle; no two
    /// pieces share land. (Overlap seen in the window was the overlay, not this.)
    #[test]
    fn every_plan_is_a_tiling() {
        use crate::args::{SnapRegions, UnitArgs};
        let (proj, req) = doc18();
        let snaps = [
            None,
            Some(SnapRegions::Cover),
            Some(SnapRegions::Fit),
            Some(SnapRegions::Merge),
        ];
        for snap in snaps {
            for margin in 0..3 {
                let mut args = args_with(UnitArgs {
                    snap_regions: snap,
                    margin_regions: margin,
                    ..Default::default()
                });
                args.ram_budget_mb = Some(4000);
                for size in [
                    UnitSize::N(1),
                    UnitSize::N(2),
                    UnitSize::N(4),
                    UnitSize::Auto,
                ] {
                    let p = plan(&proj, &req, &args, size).unwrap();
                    let area: u64 = p.units.iter().map(WorkUnit::chunks).sum();
                    let [x0, z0, x1, z1] = r(&p.rect);
                    let want = ((x1 - x0 + 1) / 16 * ((z1 - z0 + 1) / 16)) as u64;
                    assert_eq!(area, want, "{snap:?} m{margin} {size}: gap or overlap");
                    for (i, a) in p.units.iter().enumerate() {
                        let [ax0, az0, ax1, az1] = r(&a.rect);
                        for b in &p.units[i + 1..] {
                            let [bx0, bz0, bx1, bz1] = r(&b.rect);
                            let apart = ax1 < bx0 || bx1 < ax0 || az1 < bz0 || bz1 < az0;
                            assert!(apart, "{snap:?} m{margin} {size}: overlap");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn merged_slivers_leave_no_thin_edge() {
        use crate::args::{SnapRegions, UnitArgs};
        let (proj, req) = doc18();
        let units = UnitArgs {
            snap_regions: Some(SnapRegions::Merge),
            ..Default::default()
        };
        let p = plan(&proj, &req, &args_with(units), UnitSize::N(2)).unwrap();
        assert_eq!(r(&p.rect), [-2000, -1120, 1999, 1119]);
        assert!(p.name.ends_with("_merge"));
        let mut want = [(1024, 1120); 4].to_vec();
        want.extend([(976, 1120); 4]);
        want.sort_unstable();
        assert_eq!(sizes(&p.units), want);
        // Still a tiling, and no region file shared.
        let area: u64 = p.units.iter().map(WorkUnit::chunks).sum();
        assert_eq!(area, 250 * 140);
        for (i, a) in p.units.iter().enumerate() {
            for b in &p.units[i + 1..] {
                let ([ax0, az0, ax1, az1], [bx0, bz0, bx1, bz1]) = (r(&a.rect), r(&b.rect));
                assert!(ax1 < bx0 || bx1 < ax0 || az1 < bz0 || bz1 < az0);
                assert!(
                    ax1.div_euclid(512) < bx0.div_euclid(512)
                        || bx1.div_euclid(512) < ax0.div_euclid(512)
                        || az1.div_euclid(512) < bz0.div_euclid(512)
                        || bz1.div_euclid(512) < az0.div_euclid(512)
                );
            }
        }
    }

    #[test]
    fn plan_leaves_out_pieces_outside_shape() {
        use crate::args::UnitArgs;
        let proj = WebMercatorProjection::new(44.435, 26.105, 1.0);
        let grid = XZBBox::rect_from_min_max(0, 0, 2047, 2047).unwrap();
        let req = llbbox_for_rect(&proj, &grid).unwrap();
        // A triangle on the grid's north-west corner, its long side from (1800, 0) to (0, 1800).
        let corner = |x: f64, z: f64| [proj.lon_for_x(x), proj.lat_for_z(z)];
        let triangle = serde_json::json!({"type": "Polygon", "coordinates": [[
            corner(0.0, 0.0), corner(1800.0, 0.0), corner(0.0, 1800.0), corner(0.0, 0.0)
        ]]});
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("triangle.geojson");
        std::fs::write(&path, triangle.to_string()).unwrap();
        let all = plan(&proj, &req, &args_with(UnitArgs::default()), UnitSize::N(1)).unwrap();
        assert_eq!(r(&all.rect), [0, 0, 2047, 2047]);
        assert_eq!(all.units.len(), 16);
        let shaped = plan(
            &proj,
            &req,
            &args_with(UnitArgs {
                plan_mask: Some(path),
                ..Default::default()
            }),
            UnitSize::N(1),
        )
        .unwrap();
        // Piece (i, j) has a column in the triangle when 512 * (i + j) < 1800.
        let mut kept: Vec<(i32, i32)> = shaped
            .units
            .iter()
            .map(|u| (u.rect.min_x() / 512, u.rect.min_z() / 512))
            .collect();
        kept.sort_unstable();
        let want: Vec<(i32, i32)> = (0..4)
            .flat_map(|i| (0..4).map(move |j| (i, j)))
            .filter(|(i, j)| i + j <= 3)
            .collect();
        assert_eq!(kept, want);
    }

    #[test]
    fn a_mask_keeps_the_pieces_it_touches() {
        use crate::args::{SnapRegions, UnitArgs};
        let (proj, req) = doc18();
        let tmp = tempfile::tempdir().unwrap();
        // The west half of the request, as GeoJSON and as .poly.
        let ring = [
            [26.07, 44.42],
            [26.104, 44.42],
            [26.104, 44.45],
            [26.07, 44.45],
            [26.07, 44.42],
        ];
        let geojson = tmp.path().join("west.geojson");
        let feature = serde_json::json!({"type": "Feature",
            "geometry": {"type": "Polygon", "coordinates": [ring]}});
        std::fs::write(&geojson, feature.to_string()).unwrap();
        let poly = tmp.path().join("west.poly");
        let lines: Vec<String> = ring
            .iter()
            .map(|p| format!("  {} {}", p[0], p[1]))
            .collect();
        std::fs::write(&poly, format!("west\n1\n{}\nEND\nEND\n", lines.join("\n"))).unwrap();
        let masked = |mask: &std::path::Path, size| {
            let args = args_with(UnitArgs {
                plan_mask: Some(mask.to_path_buf()),
                snap_regions: Some(SnapRegions::Cover),
                ..Default::default()
            });
            plan(&proj, &req, &args, size)
        };
        let all = plan(
            &proj,
            &req,
            &args_with(UnitArgs {
                snap_regions: Some(SnapRegions::Cover),
                ..Default::default()
            }),
            UnitSize::N(1),
        )
        .unwrap();
        let west = masked(&geojson, UnitSize::N(1)).unwrap();
        // x -2048..2047 in 512-block columns: the polygon ends just west of x 0.
        assert_eq!(west.units.len() * 2, all.units.len());
        assert!(west.units.iter().all(|u| u.rect.max_x() < 0));
        assert!(west.name.contains("_m"));
        assert!(west
            .units
            .iter()
            .enumerate()
            .all(|(i, u)| u.index as usize == i));
        let from_poly = masked(&poly, UnitSize::N(1)).unwrap();
        let rects = |p: &Plan| p.units.iter().map(|u| r(&u.rect)).collect::<Vec<_>>();
        assert_eq!(rects(&from_poly), rects(&west));
        // Mixed sizes split what the mask only partly holds.
        let auto = masked(&geojson, UnitSize::Auto).unwrap();
        assert!(auto.units.iter().all(|u| u.rect.max_x() < 0));
        // A polygon elsewhere holds nothing.
        let far = tmp.path().join("far.geojson");
        let far_ring = serde_json::json!({"type": "Polygon",
            "coordinates": [[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]]});
        std::fs::write(&far, far_ring.to_string()).unwrap();
        assert!(masked(&far, UnitSize::N(1)).is_err());
        std::fs::write(&far, "not a polygon").unwrap();
        assert!(masked(&far, UnitSize::N(1)).is_err());
    }

    #[test]
    fn pieces_build_in_the_order_asked() {
        use crate::args::PieceOrder;
        let (proj, world) = bucharest();
        let units = plan_units(&proj, &world, 1).unwrap();
        let n = units.len();
        let all = |o: Option<&PieceOrder>| build_order(&units, o).unwrap();
        assert_eq!(all(None), (0..n).collect::<Vec<_>>());
        assert_eq!(all(Some(&PieceOrder::Centre)), all(None));
        for (o, key) in [
            (
                PieceOrder::West,
                (|u: &WorkUnit| u.rect.min_x()) as fn(&WorkUnit) -> i32,
            ),
            (PieceOrder::East, |u| -u.rect.max_x()),
            (PieceOrder::North, |u| u.rect.min_z()),
            (PieceOrder::South, |u| -u.rect.max_z()),
        ] {
            let got = all(Some(&o));
            assert_eq!(got.len(), n);
            assert!(
                got.windows(2)
                    .all(|w| key(&units[w[0]]) <= key(&units[w[1]])),
                "{o:?}"
            );
        }
        let got = all(Some(&PieceOrder::List(vec![3, 1, 3])));
        assert_eq!(&got[..3], &[2, 0, 1]);
        assert_eq!(got.len(), n);
        assert!(build_order(&units, Some(&PieceOrder::List(vec![n as u32 + 1]))).is_err());
    }

    #[test]
    fn plan_units_is_deterministic() {
        let (proj, world) = bucharest();
        let key = |u: &[WorkUnit]| u.iter().map(|u| (u.index, r(&u.rect))).collect::<Vec<_>>();
        let a = plan_units(&proj, &world, 1).unwrap();
        assert_eq!(key(&a), key(&plan_units(&proj, &world, 1).unwrap()));
        assert!(a.iter().enumerate().all(|(i, u)| u.index as usize == i));
    }

    #[test]
    fn a_piece_bbox_snaps_back_to_its_rect() {
        let (proj, world) = bucharest();
        for u in plan_units(&proj, &world, 1).unwrap() {
            let parsed = LLBBox::from_str(&u.bbox.arg()).unwrap();
            assert_eq!(parsed, u.bbox);
            assert_eq!(
                r(&snap_bbox_to_chunks(&proj, &parsed).unwrap().0),
                r(&u.rect)
            );
        }
    }

    /// The overlay draws `bounds`, so it must sit where `rect` does: pieces meet on region
    /// boundaries, tile the job, and `bounds` projects back to `rect` within a block.
    #[test]
    fn the_piece_grid_is_what_the_overlay_draws() {
        // The second is thin and far from the projection's origin.
        let reqs = [
            LLBBox::new(44.40, 26.00, 44.52, 26.22).unwrap(),
            LLBBox::new(44.80, 25.30, 44.82, 25.90).unwrap(),
        ];
        for (req, scale) in reqs
            .iter()
            .flat_map(|r| [1.0, 0.5, 0.25, 0.05].map(|s| (r, s)))
        {
            let proj = WebMercatorProjection::new(44.46, 26.11, scale);
            let world = snap_bbox_to_chunks(&proj, req).unwrap().0;
            if scale == 0.05 && req.min().lat() > 44.7 {
                assert!(plan_units(&proj, &world, 1).unwrap().len() > 1);
            }
            for n in [1, 2] {
                let side = 512 * n;
                let units = plan_units(&proj, &world, n).unwrap();
                let mut area = 0u64;
                for (i, a) in units.iter().enumerate() {
                    let [x0, z0, x1, z1] = r(&a.rect);
                    assert!(
                        x0 >= world.min_x() && x1 <= world.max_x(),
                        "s={scale} n={n}"
                    );
                    assert!(
                        z0 >= world.min_z() && z1 <= world.max_z(),
                        "s={scale} n={n}"
                    );
                    // Interior edges are region boundaries; the job's own edges are chunks.
                    assert!(x0 == world.min_x() || x0 % side == 0, "s={scale} n={n}");
                    assert!(z0 == world.min_z() || z0 % side == 0, "s={scale} n={n}");
                    assert!(
                        x1 == world.max_x() || (x1 + 1) % side == 0,
                        "s={scale} n={n}"
                    );
                    assert!(
                        z1 == world.max_z() || (z1 + 1) % side == 0,
                        "s={scale} n={n}"
                    );
                    area += a.chunks();
                    for b in &units[i + 1..] {
                        let [bx0, bz0, bx1, bz1] = r(&b.rect);
                        assert!(
                            x1 < bx0 || bx1 < x0 || z1 < bz0 || bz1 < z0,
                            "s={scale} n={n}"
                        );
                    }
                    let (lo, hi) = (a.bbox.min(), a.bbox.max());
                    let back = [
                        proj.x_for_lon(lo.lng()),
                        proj.z_for_lat(hi.lat()),
                        proj.x_for_lon(hi.lng()) - 1.0,
                        proj.z_for_lat(lo.lat()) - 1.0,
                    ];
                    for (b, want) in back.iter().zip([x0, z0, x1, z1]) {
                        assert!(
                            (b - f64::from(want)).abs() <= 1.0,
                            "s={scale} n={n}: {b} vs {want}"
                        );
                    }
                }
                let (w, h) = (
                    world.max_x() - world.min_x() + 1,
                    world.max_z() - world.min_z() + 1,
                );
                assert_eq!(
                    area,
                    (w as u64 / 16) * (h as u64 / 16),
                    "s={scale} n={n}: gap"
                );
            }
        }
    }
}
