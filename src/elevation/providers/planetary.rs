//! Moon (LRO LOLA) and Mars (MGS MOLA) elevation from the NASA PDS archives.
//!
//! Both are flat, uncompressed, fixed-record-length rasters served with
//! `accept-ranges: bytes`, so a window costs one range request per source row
//! rather than the whole file. No tile pyramid, no decoder, no reprojection.

use crate::celestial::CelestialBody;
use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::cache::get_cache_dir;
use crate::elevation::provider::{ElevationProvider, RawElevationGrid};
use rayon::prelude::*;
use std::io::Read;
use std::path::PathBuf;

/// Source pixels per axis, so really a request budget. Above it the window is
/// strided, which at these coarse scales costs detail nobody could see.
const MAX_SOURCE_DIM: usize = 3072;
/// Static files, so the fetch is latency-bound; sockets are what make it quick.
const MAX_CONCURRENT_DOWNLOADS: usize = 24;
const ROW_MAX_RETRIES: u32 = 3;
const ROW_RETRY_BASE_DELAY_MS: u64 = 400;

/// A flat PDS raster split into a regular lat/lon tile grid.
struct DemSpec {
    base_url: &'static str,
    /// Pixels per degree, both axes.
    ppd: usize,
    /// Tile extent in degrees.
    lat_span: f64,
    lon_span: f64,
    /// Latitude coverage of the product as a whole.
    lat_min: f64,
    lat_max: f64,
    /// PDS `SAMPLE_TYPE`: MSB_INTEGER for MOLA, LSB_INTEGER for LOLA.
    big_endian: bool,
    /// PDS `SCALING_FACTOR`: metres per DN.
    dn_to_meters: f64,
}

impl DemSpec {
    fn for_body(body: CelestialBody) -> Option<Self> {
        match body {
            // megt*hb: 16 tiles of 44x90 deg, 128 ppd (463 m/px), metres above the
            // areoid. Beyond +-88 is a polar-stereographic product we do not read.
            CelestialBody::Mars => Some(Self {
                base_url:
                    "https://pds-geosciences.wustl.edu/mgs/mgs-m-mola-5-megdr-l3-v1/mgsl_300x/meg128/",
                ppd: 128,
                lat_span: 44.0,
                lon_span: 90.0,
                lat_min: -88.0,
                lat_max: 88.0,
                big_endian: true,
                dn_to_meters: 1.0,
            }),
            // ldem_128: whole Moon in one file, 128 ppd (237 m/px), DN x 0.5 m off
            // the 1737.4 km sphere. Matched to the 200 m block; the 1024 ppd tiled
            // set costs six times the bandwidth for identical output.
            CelestialBody::Moon => Some(Self {
                base_url:
                    "https://pds-geosciences.wustl.edu/lro/lro-l-lola-3-rdr-v1/lrolol_1xxx/data/lola_gdr/cylindrical/img/",
                ppd: 128,
                lat_span: 180.0,
                lon_span: 360.0,
                lat_min: -90.0,
                lat_max: 90.0,
                big_endian: false,
                dn_to_meters: 0.5,
            }),
            CelestialBody::Earth => None,
        }
    }

    fn tile_lines(&self) -> usize {
        (self.lat_span * self.ppd as f64) as usize
    }

    fn tile_samples(&self) -> usize {
        (self.lon_span * self.ppd as f64) as usize
    }

    /// File name for the tile whose band starts at `(lat_band_min, lon_band_min)`.
    fn tile_name(&self, body: CelestialBody, lat_band_min: f64, lon_band_min: f64) -> String {
        let lat_band_max = lat_band_min + self.lat_span;
        match body {
            // megt{max_lat}{n|s}{lon:03}hb.img, label is MAXIMUM_LATITUDE.
            CelestialBody::Mars => {
                let hemi = if lat_band_max >= 0.0 { 'n' } else { 's' };
                format!(
                    "megt{:02}{hemi}{:03}hb.img",
                    lat_band_max.abs() as i32,
                    lon_band_min as i32
                )
            }
            // One global file, so the band arguments are always the full globe.
            CelestialBody::Moon => "ldem_128.img".to_string(),
            CelestialBody::Earth => unreachable!("Earth has no PDS raster"),
        }
    }
}

pub struct PlanetaryDem {
    pub body: CelestialBody,
}

impl ElevationProvider for PlanetaryDem {
    fn name(&self) -> &'static str {
        match self.body {
            CelestialBody::Moon => "lola",
            CelestialBody::Mars => "mola",
            CelestialBody::Earth => "planetary",
        }
    }

    fn coverage_bboxes(&self) -> Option<Vec<LLBBox>> {
        None
    }

    fn native_resolution_m(&self) -> f64 {
        let Some(spec) = DemSpec::for_body(self.body) else {
            return f64::MAX;
        };
        // Degree of latitude in metres, divided by pixels per degree.
        (self.body.radius_m() * std::f64::consts::PI / 180.0) / spec.ppd as f64
    }

    fn fetch_raw(
        &self,
        bbox: &LLBBox,
        grid_width: usize,
        grid_height: usize,
    ) -> Result<RawElevationGrid, Box<dyn std::error::Error>> {
        let spec = DemSpec::for_body(self.body)
            .ok_or_else(|| format!("No PDS raster for {:?}", self.body))?;

        if bbox.min().lat() < spec.lat_min || bbox.max().lat() > spec.lat_max {
            return Err(format!(
                "{} elevation covers {}..{} degrees latitude; this area reaches outside it",
                self.name(),
                spec.lat_min,
                spec.lat_max
            )
            .into());
        }

        println!(
            "Using {} ({:.0} m/px, NASA PDS); 1 block = {:.0} m, {:.1}x vertical, {:.0} m of relief fits vanilla height",
            self.name(),
            self.native_resolution_m(),
            self.body.meters_per_block(),
            self.body.vertical_exaggeration(),
            self.body.vanilla_relief_headroom_m(),
        );

        let window = SourceWindow::plan(&spec, bbox, grid_width, grid_height);
        let cache_dir = get_cache_dir(self.name());
        std::fs::create_dir_all(&cache_dir)?;

        let samples = window.fetch(&spec, self.body, &cache_dir)?;
        let heights = window.resample(bbox, &samples, grid_width, grid_height, self.body);

        Ok(RawElevationGrid {
            heights_meters: heights,
        })
    }
}

/// Longitude in the archives' 0..360 east convention.
fn lon_east(lng: f64) -> f64 {
    let wrapped = lng % 360.0;
    if wrapped < 0.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// A strided rectangle of global source pixels. Line 0 is +90 latitude and
/// sample 0 is 0 longitude, so it is independent of which tiles back it.
struct SourceWindow {
    line0: usize,
    samp0: usize,
    /// Pixels kept per axis after striding.
    lines: usize,
    samples: usize,
    step: usize,
    ppd: usize,
}

impl SourceWindow {
    fn plan(spec: &DemSpec, bbox: &LLBBox, grid_width: usize, grid_height: usize) -> Self {
        let ppd = spec.ppd as f64;
        let lon_w = lon_east(bbox.min().lng());
        let lon_e_raw = lon_east(bbox.max().lng());
        // Antimeridian crossings wrap to a lower east edge; widen instead.
        let lon_e = if lon_e_raw < lon_w {
            lon_e_raw + 360.0
        } else {
            lon_e_raw
        };

        let line0 = ((90.0 - bbox.max().lat()) * ppd).floor().max(0.0) as usize;
        let line1 = ((90.0 - bbox.min().lat()) * ppd).ceil() as usize;
        let samp0 = (lon_w * ppd).floor().max(0.0) as usize;
        let samp1 = (lon_e * ppd).ceil() as usize;

        let raw_lines = (line1 - line0).max(1);
        let raw_samples = (samp1 - samp0).max(1);

        // One source pixel per grid cell is all the grid can show.
        let by_grid = (raw_lines as f64 / grid_height.max(1) as f64)
            .max(raw_samples as f64 / grid_width.max(1) as f64)
            .floor();
        // Ceil, not floor: almost meeting the budget still leaves it over.
        let by_budget = ((raw_lines.max(raw_samples) as f64) / MAX_SOURCE_DIM as f64).ceil();
        let step = by_grid.max(by_budget).max(1.0) as usize;

        Self {
            line0,
            samp0,
            lines: raw_lines.div_ceil(step),
            samples: raw_samples.div_ceil(step),
            step,
            ppd: spec.ppd,
        }
    }

    fn cache_path(&self, dir: &std::path::Path) -> PathBuf {
        dir.join(format!(
            "p{}_{}_{}_{}x{}_s{}.bin",
            self.ppd, self.line0, self.samp0, self.lines, self.samples, self.step
        ))
    }

    /// Decoded metres, row-major, `lines * samples`. NaN marks a gap.
    fn fetch(
        &self,
        spec: &DemSpec,
        body: CelestialBody,
        cache_dir: &std::path::Path,
    ) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        let cached = self.cache_path(cache_dir);
        if let Ok(bytes) = std::fs::read(&cached) {
            if bytes.len() == self.lines * self.samples * 4 {
                println!("Reusing cached {} window", body.as_str());
                return Ok(bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect());
            }
        }

        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!(
                "Arnis/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/louis-e/arnis)"
            ))
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(MAX_CONCURRENT_DOWNLOADS)
            .build()
            .map_err(|e| format!("Failed to create PDS fetch thread pool: {e}"))?;

        println!(
            "Fetching {} source rows from NASA PDS ({} px per row)...",
            self.lines, self.samples
        );

        let rows: Vec<Result<Vec<f32>, String>> = pool.install(|| {
            (0..self.lines)
                .into_par_iter()
                .map(|r| self.fetch_row(&client, spec, body, r))
                .collect()
        });

        let mut out = Vec::with_capacity(self.lines * self.samples);
        let mut failed = 0usize;
        for row in rows {
            match row {
                Ok(values) => out.extend_from_slice(&values),
                Err(e) => {
                    if failed == 0 {
                        eprintln!("Warning: PDS row fetch failed: {e}");
                    }
                    failed += 1;
                    // Post-processing fills NaN, so a dropped row is a seam.
                    out.extend(std::iter::repeat_n(f32::NAN, self.samples));
                }
            }
        }
        if failed == self.lines {
            return Err(format!("All {} PDS row requests failed", self.lines).into());
        }
        if failed > 0 {
            eprintln!("Warning: {failed} of {} PDS rows unavailable", self.lines);
        }

        let bytes: Vec<u8> = out.iter().flat_map(|v| v.to_le_bytes()).collect();
        if let Err(e) = std::fs::write(&cached, &bytes) {
            eprintln!("Warning: could not cache PDS window: {e}");
        }
        Ok(out)
    }

    /// One strided output row, assembled from every tile it crosses.
    fn fetch_row(
        &self,
        client: &reqwest::blocking::Client,
        spec: &DemSpec,
        body: CelestialBody,
        row: usize,
    ) -> Result<Vec<f32>, String> {
        let global_line = self.line0 + row * self.step;
        let lat = 90.0 - global_line as f64 / self.ppd as f64;
        // A line exactly on a band edge belongs to the band below it.
        let lat_band_min = (((lat - f64::EPSILON) / spec.lat_span).floor() * spec.lat_span)
            .clamp(spec.lat_min, spec.lat_max - spec.lat_span);
        let tile_line = ((lat_band_min + spec.lat_span - lat) * spec.ppd as f64).round() as usize;
        let tile_line = tile_line.min(spec.tile_lines() - 1);

        let mut out = Vec::with_capacity(self.samples);
        let mut col = 0usize;
        while col < self.samples {
            let global_samp = (self.samp0 + col * self.step) % (360 * self.ppd);
            let lon = global_samp as f64 / self.ppd as f64;
            let lon_band_min = (lon / spec.lon_span).floor() * spec.lon_span;
            let tile_samp0 = ((lon - lon_band_min) * spec.ppd as f64).round() as usize;

            // How many further strided columns stay inside this tile.
            let remaining_in_tile = spec.tile_samples().saturating_sub(tile_samp0);
            let take = remaining_in_tile
                .div_ceil(self.step)
                .min(self.samples - col);
            let byte_len = ((take - 1) * self.step + 1) * 2;

            let name = spec.tile_name(body, lat_band_min, lon_band_min);
            let offset = (tile_line * spec.tile_samples() + tile_samp0) * 2;
            let bytes = range_get(
                client,
                &format!("{}{name}", spec.base_url),
                offset as u64,
                byte_len,
            )?;

            for i in 0..take {
                let b = i * self.step * 2;
                let dn = if spec.big_endian {
                    i16::from_be_bytes([bytes[b], bytes[b + 1]])
                } else {
                    i16::from_le_bytes([bytes[b], bytes[b + 1]])
                };
                // PDS marks gaps with the type minimum.
                out.push(if dn == i16::MIN {
                    f32::NAN
                } else {
                    (dn as f64 * spec.dn_to_meters) as f32
                });
            }
            col += take;
        }
        Ok(out)
    }

    /// Bilinear-sample onto the output grid, scaling metres by the terrain gain.
    fn resample(
        &self,
        bbox: &LLBBox,
        samples: &[f32],
        grid_width: usize,
        grid_height: usize,
        body: CelestialBody,
    ) -> Vec<Vec<f64>> {
        let gain = body.terrain_gain();
        let ppd = self.ppd as f64;
        let lon_w = lon_east(bbox.min().lng());
        let lon_e_raw = lon_east(bbox.max().lng());
        let lon_e = if lon_e_raw < lon_w {
            lon_e_raw + 360.0
        } else {
            lon_e_raw
        };
        let (lat_n, lat_s) = (bbox.max().lat(), bbox.min().lat());

        let at = |li: usize, si: usize| -> f32 {
            samples[li.min(self.lines - 1) * self.samples + si.min(self.samples - 1)]
        };

        (0..grid_height)
            .into_par_iter()
            .map(|gz| {
                // Grid row 0 is the north edge, matching the raster's line order.
                let t = if grid_height > 1 {
                    gz as f64 / (grid_height - 1) as f64
                } else {
                    0.0
                };
                let lat = lat_n + (lat_s - lat_n) * t;
                let fl = (((90.0 - lat) * ppd) - self.line0 as f64) / self.step as f64;
                let (l0, lf) = split(fl, self.lines);

                (0..grid_width)
                    .map(|gx| {
                        let u = if grid_width > 1 {
                            gx as f64 / (grid_width - 1) as f64
                        } else {
                            0.0
                        };
                        let lon = lon_w + (lon_e - lon_w) * u;
                        let fs = ((lon * ppd) - self.samp0 as f64) / self.step as f64;
                        let (s0, sf) = split(fs, self.samples);

                        let v = bilinear(
                            at(l0, s0),
                            at(l0, s0 + 1),
                            at(l0 + 1, s0),
                            at(l0 + 1, s0 + 1),
                            sf,
                            lf,
                        );
                        v as f64 * gain
                    })
                    .collect()
            })
            .collect()
    }
}

/// Integer index plus fraction, clamped so `idx + 1` stays addressable.
fn split(v: f64, len: usize) -> (usize, f64) {
    let clamped = v.clamp(0.0, (len.saturating_sub(1)) as f64);
    let idx = clamped.floor() as usize;
    (idx.min(len.saturating_sub(1)), clamped - idx as f64)
}

/// Falls back to the mean of the finite corners, so one gap pixel does not punch
/// a NaN hole into good terrain.
fn bilinear(a: f32, b: f32, c: f32, d: f32, u: f64, v: f64) -> f32 {
    let corners = [
        (a, (1.0 - u) * (1.0 - v)),
        (b, u * (1.0 - v)),
        (c, (1.0 - u) * v),
        (d, u * v),
    ];
    let mut sum = 0.0;
    let mut weight = 0.0;
    for (value, w) in corners {
        if value.is_finite() {
            sum += value as f64 * w;
            weight += w;
        }
    }
    if weight > 0.0 {
        (sum / weight) as f32
    } else {
        f32::NAN
    }
}

fn range_get(
    client: &reqwest::blocking::Client,
    url: &str,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, String> {
    let range = format!("bytes={}-{}", offset, offset + len as u64 - 1);
    let mut last = String::new();
    for attempt in 0..ROW_MAX_RETRIES {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(
                ROW_RETRY_BASE_DELAY_MS * (1 << (attempt - 1)),
            ));
        }
        let _permit = crate::net::request_permit();
        match client.get(url).header("Range", &range).send() {
            Ok(resp) => {
                let status = resp.status();
                // A 200 means the server ignored Range and is sending the whole file.
                if status.as_u16() != 206 {
                    last = format!("{url}: expected 206, got {status}");
                    continue;
                }
                let mut buf = Vec::with_capacity(len);
                match resp.take(len as u64).read_to_end(&mut buf) {
                    Ok(_) if buf.len() == len => return Ok(buf),
                    Ok(_) => last = format!("{url}: short read ({} of {len})", buf.len()),
                    Err(e) => last = format!("{url}: {e}"),
                }
            }
            Err(e) => last = format!("{url}: {e}"),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mars_tile_names_match_the_archive() {
        let spec = DemSpec::for_body(CelestialBody::Mars).unwrap();
        let b = CelestialBody::Mars;
        // Verified against the meg128 directory listing.
        assert_eq!(spec.tile_name(b, 0.0, 180.0), "megt44n180hb.img");
        assert_eq!(spec.tile_name(b, 44.0, 0.0), "megt88n000hb.img");
        assert_eq!(spec.tile_name(b, -44.0, 0.0), "megt00n000hb.img");
        assert_eq!(spec.tile_name(b, -88.0, 270.0), "megt44s270hb.img");
    }

    #[test]
    fn moon_uses_the_single_global_file() {
        let spec = DemSpec::for_body(CelestialBody::Moon).unwrap();
        let b = CelestialBody::Moon;
        assert_eq!(spec.tile_name(b, -90.0, 0.0), "ldem_128.img");
        // One tile covering the globe, so any coordinate resolves to it.
        assert_eq!(spec.tile_name(b, -90.0, 300.0), "ldem_128.img");
    }

    #[test]
    fn tile_dimensions_match_the_labels() {
        let mars = DemSpec::for_body(CelestialBody::Mars).unwrap();
        assert_eq!((mars.tile_lines(), mars.tile_samples()), (5632, 11520));
        let moon = DemSpec::for_body(CelestialBody::Moon).unwrap();
        assert_eq!((moon.tile_lines(), moon.tile_samples()), (23040, 46080));
    }

    #[test]
    fn native_resolutions_are_right() {
        let mars = PlanetaryDem {
            body: CelestialBody::Mars,
        };
        assert!((mars.native_resolution_m() - 463.0).abs() < 2.0);
        let moon = PlanetaryDem {
            body: CelestialBody::Moon,
        };
        assert!((moon.native_resolution_m() - 237.0).abs() < 2.0);
    }

    #[test]
    fn window_strides_down_to_the_grid() {
        let spec = DemSpec::for_body(CelestialBody::Moon).unwrap();
        // 8 degrees at 128 ppd is 1024 source pixels; a 256-cell grid strides 4x.
        let bbox = LLBBox::new(20.0, 10.0, 28.0, 18.0).unwrap();
        let w = SourceWindow::plan(&spec, &bbox, 256, 256);
        assert_eq!(w.step, 4);
        assert!(w.lines <= 257 && w.samples <= 257);
    }

    #[test]
    fn window_never_exceeds_the_request_budget() {
        let spec = DemSpec::for_body(CelestialBody::Moon).unwrap();
        let bbox = LLBBox::new(-60.0, -100.0, 60.0, 100.0).unwrap();
        let w = SourceWindow::plan(&spec, &bbox, 100_000, 100_000);
        assert!(w.lines <= MAX_SOURCE_DIM && w.samples <= MAX_SOURCE_DIM);
    }

    #[test]
    fn bilinear_ignores_gap_corners() {
        assert_eq!(bilinear(10.0, f32::NAN, 10.0, f32::NAN, 0.5, 0.5), 10.0);
        assert!(bilinear(f32::NAN, f32::NAN, f32::NAN, f32::NAN, 0.5, 0.5).is_nan());
    }

    #[test]
    fn longitude_wraps_to_east() {
        assert_eq!(lon_east(-20.0), 340.0);
        assert_eq!(lon_east(20.0), 20.0);
        assert_eq!(lon_east(180.0), 180.0);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Hits the real archive; ignored by default so offline runs stay green.
    #[test]
    #[ignore]
    fn moon_window_matches_known_terrain() {
        let dem = PlanetaryDem {
            body: CelestialBody::Moon,
        };
        // Mare Vaporum area: flat mare a couple of km below the reference sphere.
        let bbox = LLBBox::new(21.5, -15.5, 22.5, -14.5).unwrap();
        let raw = dem.fetch_raw(&bbox, 64, 64).unwrap();
        let gain = CelestialBody::Moon.terrain_gain();
        let vals: Vec<f64> = raw
            .heights_meters
            .iter()
            .flatten()
            .map(|v| v / gain)
            .collect();
        let lo = vals.iter().cloned().fold(f64::MAX, f64::min);
        let hi = vals.iter().cloned().fold(f64::MIN, f64::max);
        println!("Moon 22N 345E: {lo:.0}..{hi:.0} m (real)");
        assert!((-2600.0..-1000.0).contains(&lo), "lo={lo}");
    }

    #[test]
    #[ignore]
    fn mars_window_finds_olympus_mons() {
        let dem = PlanetaryDem {
            body: CelestialBody::Mars,
        };
        // Olympus Mons summit caldera, 18.65N 226.2E.
        let bbox = LLBBox::new(18.4, 226.2 - 360.0, 18.9, 226.7 - 360.0).unwrap();
        let raw = dem.fetch_raw(&bbox, 64, 64).unwrap();
        let gain = CelestialBody::Mars.terrain_gain();
        let hi = raw
            .heights_meters
            .iter()
            .flatten()
            .map(|v| v / gain)
            .fold(f64::MIN, f64::max);
        println!("Mars Olympus Mons peak in window: {hi:.0} m (real)");
        assert!((18_000.0..22_000.0).contains(&hi), "hi={hi}");
    }
}
