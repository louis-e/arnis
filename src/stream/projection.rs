//! Anchor model for stream mode: the mapping between the real world and Minecraft.
//!
//! An **anchor** pins one real-world position (`lat`, `lon`) to one Minecraft
//! position (`mc_x`, `mc_z`) and claims a circular patch of `radius_m` around
//! it. Inside that patch, geometry is generated through a
//! [`TransverseMercatorProjection`] centred on the anchor, so a metre of ground
//! is exactly `scale` blocks (one, at the default 1:1) — no `1/cos(latitude)`
//! oversizing.
//!
//! Two projections are involved, and they do different jobs:
//!
//! * **Placement** (where in the world a *new* patch goes) uses the
//!   fixed-origin Web Mercator plane — origin `(0, 0)`, the true global
//!   EPSG:3857-style plane. It is globally consistent: every point on Earth has
//!   exactly one position on it, so anchors placed independently keep their
//!   relative geography. Its scale distortion does not matter here because it
//!   is only used to choose a patch *centre*.
//! * **Generation** (what goes where *inside* a patch) uses the anchor's own
//!   transverse Mercator, which is true to scale near its own central meridian.
//!
//! Because the two disagree, patch separation has to be checked in *both*
//! spaces; see [`AnchorSet::place_new`].
//!
//! Orientation follows the rest of the crate: increasing X is east, north is
//! **negative** Z.
//!
//! # Units
//!
//! Two units live side by side in here, and they are only the same at
//! `scale == 1.0`:
//!
//! * `lat`/`lon`/`radius_m` and everything derived from a great-circle
//!   distance are in **metres** of real ground.
//! * `mc_x`/`mc_z` and everything compared against them are in **blocks**.
//!
//! [`Anchor::scale`] is the blocks-per-metre factor that converts one into the
//! other, so a metre-denominated radius becomes a block-denominated one through
//! [`Anchor::radius_blocks`]. Geographic checks stay in metres; Minecraft-space
//! checks convert first.

// This module is the anchor vocabulary for the whole stream layer; individual
// accessors are consumed by `session`, `tiles` and `mod` rather than here.
#![allow(dead_code)]

use crate::args::validate_scale;
use crate::projection::{
    Projection, TransverseMercatorProjection, WebMercatorProjection, EARTH_RADIUS,
};

/// Default radius of a patch, in real-world metres, when a client does not ask
/// for a specific one.
///
/// 500 km is a deliberate compromise: large enough that a whole country-sized
/// region lives under one anchor, small enough that the anchor's transverse
/// Mercator stays inside a few percent of true scale at its rim.
pub const DEFAULT_ANCHOR_RADIUS_M: f64 = 500_000.0;

/// Hard upper bound on a patch radius, in real-world metres.
///
/// An anchor owns its own central meridian and is only true to scale near it:
/// the transverse Mercator scale factor is ~1.003 at 500 km, ~1.05 at 2000 km
/// and diverges from there, so a patch much larger than this silently oversizes
/// everything at its rim. `AddAnchor` has always enforced this cap; anchor
/// validation now applies it to handshake-supplied anchors too, so the two
/// paths cannot drift apart.
pub const MAX_ANCHOR_RADIUS_M: f64 = 500_000.0;

/// Grid that a newly placed anchor's Minecraft position is snapped to, in
/// blocks.
///
/// Snapping keeps world coordinates round and human-readable ("your city is at
/// x = 1,300,000") and keeps two nearby-but-distinct requests from producing
/// two anchors a handful of blocks apart. The price is that a placement can
/// move by up to `ANCHOR_GRID_M / 2` on each axis, which is one of the reasons
/// Minecraft-space separation is verified independently of geographic
/// separation.
pub const ANCHOR_GRID_M: f64 = 100_000.0;

/// Web Mercator diverges at the poles; refuse to place anchors beyond the
/// conventional EPSG:3857 cutoff.
const MAX_PLACEMENT_LAT: f64 = 85.0;

/// One real-world position nailed to one Minecraft position, owning a circular
/// patch around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// Client-visible identifier, unique within an [`AnchorSet`].
    pub id: u32,
    /// Latitude of the patch centre, in degrees.
    pub lat: f64,
    /// Longitude of the patch centre, in degrees.
    pub lon: f64,
    /// Minecraft X the centre is pinned to.
    pub mc_x: i32,
    /// Minecraft Z the centre is pinned to.
    pub mc_z: i32,
    /// Patch radius in real-world **metres**. In blocks it is
    /// [`Anchor::radius_blocks`], which is the same number only at
    /// `scale == 1.0`.
    pub radius_m: f64,
    /// Blocks per metre this patch is generated at: the session's
    /// `GenConfig.scale`.
    ///
    /// It lives on the anchor because the anchor *is* the real-world-to-world
    /// mapping, and that mapping is scale-dependent. The generator sizes every
    /// feature by the session scale (road widths, building heights, tree radii),
    /// so the geometry those features are drawn onto has to be projected at the
    /// same factor — see [`Anchor::projection`]. All anchors of one session
    /// carry that session's scale.
    pub scale: f64,
}

impl Anchor {
    /// Build an anchor directly from its parts, without validating it against
    /// any other anchor. Use [`AnchorSet::new`] or [`AnchorSet::place_new`]
    /// when the anchor has to coexist with others.
    pub fn new(
        id: u32,
        lat: f64,
        lon: f64,
        mc_x: i32,
        mc_z: i32,
        radius_m: f64,
        scale: f64,
    ) -> Self {
        Self {
            id,
            lat,
            lon,
            mc_x,
            mc_z,
            radius_m,
            scale,
        }
    }

    /// The patch radius in **blocks**: `radius_m` metres at [`Anchor::scale`]
    /// blocks per metre.
    ///
    /// Every Minecraft-space comparison goes through here, because `radius_m` is
    /// metres while `mc_x`/`mc_z` are blocks and the two differ at any scale
    /// other than 1.0. Great-circle comparisons stay in metres and must *not*
    /// use this.
    pub fn radius_blocks(&self) -> f64 {
        self.radius_m * self.scale
    }

    /// The projection to generate this patch with.
    ///
    /// A transverse Mercator centred on the anchor's own `(lat, lon)`, with the
    /// false easting/northing set to the anchor's Minecraft position, so
    /// [`Projection::forward`] yields **absolute** Minecraft coordinates —
    /// `forward(self.lat, self.lon)` is exactly `(mc_x, mc_z)`.
    ///
    /// The blocks-per-metre factor is [`Anchor::scale`], not a hardcoded 1.0.
    /// The generation pipeline sizes every feature by the session scale — a
    /// 10 m carriageway is painted 20 blocks wide at `scale = 2.0` — so node
    /// positions have to be projected at the same factor, or oversized features
    /// are drawn onto a 1:1 street grid.
    ///
    /// The consequence is deliberate: the same lat/lon lands on different blocks
    /// for sessions generated at different scales. A world is generated at one
    /// scale, and its anchors record which.
    pub fn projection(&self) -> TransverseMercatorProjection {
        TransverseMercatorProjection::with_origin(
            self.lat,
            self.lon,
            self.scale,
            f64::from(self.mc_x),
            f64::from(self.mc_z),
        )
    }

    /// Whether `(lat, lon)` lies inside this patch, by great-circle distance.
    ///
    /// The rim is **exclusive** (`distance < radius_m`), which pairs with the
    /// overlap rule in [`AnchorSet`]: two anchors exactly `r1 + r2` apart are
    /// allowed to touch precisely because no point is then inside both.
    pub fn contains_latlon(&self, lat: f64, lon: f64) -> bool {
        great_circle_distance_m(self.lat, self.lon, lat, lon) < self.radius_m
    }

    /// Whether the Minecraft column `(x, z)` lies inside this patch, by
    /// Euclidean distance from `(mc_x, mc_z)`. The rim is exclusive, as in
    /// [`Anchor::contains_latlon`].
    ///
    /// Both sides of the comparison are **blocks**, so the radius is converted
    /// through [`Anchor::radius_blocks`] first.
    pub fn contains_mc(&self, x: i32, z: i32) -> bool {
        mc_distance(self.mc_x, self.mc_z, x, z) < self.radius_blocks()
    }
}

/// A validated collection of anchors: unique ids, and no two patches
/// overlapping in either geographic or Minecraft space.
#[derive(Debug, Clone, Default)]
pub struct AnchorSet {
    anchors: Vec<Anchor>,
}

impl AnchorSet {
    /// Validate a set of anchors handed over wholesale — this is the `Hello`
    /// handshake path, where the mod reports the anchors its world already
    /// contains.
    ///
    /// Rejects duplicate ids, individually malformed anchors, and any pair that
    /// overlaps. Client-supplied `mcX`/`mcZ` are taken at face value: the mod
    /// owns its world, and its anchors need not sit where this crate's
    /// placement would have put them. They still have to be mutually
    /// consistent, which is exactly what the overlap check enforces.
    ///
    /// "Malformed" includes a radius above [`MAX_ANCHOR_RADIUS_M`]: the
    /// handshake gets exactly the cap `AddAnchor` applies, so a client cannot
    /// declare a continent-sized patch by announcing it instead of asking for
    /// it.
    pub fn new(anchors: Vec<Anchor>) -> Result<Self, String> {
        for anchor in &anchors {
            validate_anchor(anchor)?;
        }

        for (i, a) in anchors.iter().enumerate() {
            for b in &anchors[i + 1..] {
                if a.id == b.id {
                    return Err(format!("duplicate anchor id {}", a.id));
                }
                if let Some(detail) = overlap_detail(a, b) {
                    return Err(format!(
                        "anchor 'id {}' would overlap 'id {}' {}",
                        b.id, a.id, detail
                    ));
                }
            }
        }

        Ok(Self { anchors })
    }

    /// All anchors, in insertion order.
    pub fn anchors(&self) -> &[Anchor] {
        &self.anchors
    }

    /// Number of anchors in the set.
    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    /// Whether the set holds no anchors.
    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    /// Look an anchor up by its id.
    pub fn get(&self, id: u32) -> Option<&Anchor> {
        self.anchors.iter().find(|a| a.id == id)
    }

    /// The anchor whose patch contains `(lat, lon)`, if any. At most one can,
    /// because the set is overlap-free.
    pub fn find_containing_latlon(&self, lat: f64, lon: f64) -> Option<&Anchor> {
        self.anchors.iter().find(|a| a.contains_latlon(lat, lon))
    }

    /// The anchor whose patch contains the Minecraft column `(x, z)`, if any.
    /// This is what turns a chunk request into "which patch am I generating?",
    /// and its `None` is the `out_of_patch` error.
    pub fn find_containing_mc(&self, x: i32, z: i32) -> Option<&Anchor> {
        self.anchors.iter().find(|a| a.contains_mc(x, z))
    }

    /// Add an already-built anchor, re-validating it against the set.
    pub fn insert(&mut self, anchor: Anchor) -> Result<(), String> {
        validate_anchor(&anchor)?;
        if self.get(anchor.id).is_some() {
            return Err(format!("duplicate anchor id {}", anchor.id));
        }
        if let Some((other, detail)) = self.first_conflict(&anchor) {
            return Err(format!("anchor would overlap 'id {}' {}", other, detail));
        }
        self.anchors.push(anchor);
        Ok(())
    }

    /// Place a new anchor for `(lat, lon)` — the core of the `AddAnchor`
    /// message.
    ///
    /// The Minecraft position comes from the **fixed-origin** Web Mercator
    /// plane (origin `0, 0`), snapped to [`ANCHOR_GRID_M`]. Using the one
    /// global plane is what makes independently added anchors keep their real
    /// relative geography: Berlin ends up east of Paris and north of Rome
    /// without anybody coordinating.
    ///
    /// The new anchor is then checked against every existing one **twice**,
    /// and the pair is rejected if *either* check finds an overlap:
    ///
    /// 1. **Geographic** — great-circle distance between the centres versus the
    ///    sum of the radii. Catches the case where the same piece of the real
    ///    world would be generated into two different places in the world.
    /// 2. **Minecraft space** — Euclidean distance between the pinned block
    ///    positions versus the same sum. Catches the case that actually
    ///    corrupts a world: two patches writing blocks over each other.
    ///
    /// Neither check subsumes the other. Web Mercator inflates distances by
    /// `1/cos(latitude)`, so a pair that overlaps geographically is usually
    /// spread *further* apart in Minecraft space — check 1 catches those.
    /// Conversely, grid snapping can pull two placements up to `ANCHOR_GRID_M`
    /// per axis closer than the ground truth, and client-supplied anchors from
    /// the handshake need not follow Web Mercator placement at all — check 2
    /// catches those. Both are tested below.
    ///
    /// Touching is allowed: an overlap is a *strictly* smaller separation than
    /// the sum of the radii, matching the exclusive rim of
    /// [`Anchor::contains_latlon`].
    ///
    /// `scale` is the session's blocks-per-metre factor. It multiplies the
    /// placement plane as well as the patch, so that a world generated at
    /// `scale = 2.0` puts its patches twice as far apart in blocks as it makes
    /// them wide: clearances stay proportional and the relative geography is
    /// unchanged. Grid snapping stays in blocks, so world coordinates stay
    /// round at any scale.
    ///
    /// The anchor is *not* added to the set; pass it to [`AnchorSet::insert`]
    /// once the caller has committed to it. It is assigned the lowest unused
    /// id.
    pub fn place_new(
        &self,
        lat: f64,
        lon: f64,
        radius_m: f64,
        scale: f64,
    ) -> Result<Anchor, String> {
        if !lat.is_finite() || !lon.is_finite() {
            return Err(format!("anchor position is not finite: ({lat}, {lon})"));
        }
        if lat.abs() > MAX_PLACEMENT_LAT {
            return Err(format!(
                "latitude {lat} is outside the Web Mercator placement range of \
                 +/-{MAX_PLACEMENT_LAT} degrees"
            ));
        }
        if lon.abs() > 180.0 {
            return Err(format!("longitude {lon} is outside +/-180 degrees"));
        }
        if !(radius_m.is_finite() && radius_m > 0.0) {
            return Err(format!("anchor radius must be positive, got {radius_m}"));
        }
        if radius_m > MAX_ANCHOR_RADIUS_M {
            return Err(format!(
                "anchor radius {radius_m} m is above the maximum patch radius of \
                 {MAX_ANCHOR_RADIUS_M} m"
            ));
        }
        validate_scale(scale)
            .map_err(|reason| format!("anchor scale {scale} is unusable: {reason}"))?;

        // The placement plane is metres; multiplying by the session scale turns
        // it into blocks, which is what `mc_x`/`mc_z` are.
        let (raw_x, raw_z) = WebMercatorProjection::new(0.0, 0.0, 1.0).forward(lat, lon);
        let candidate = Anchor {
            id: self.next_id(),
            lat,
            lon,
            mc_x: snap_to_grid(raw_x * scale),
            mc_z: snap_to_grid(raw_z * scale),
            radius_m,
            scale,
        };

        if let Some((other, detail)) = self.first_conflict(&candidate) {
            return Err(format!("anchor would overlap 'id {other}' {detail}"));
        }

        Ok(candidate)
    }

    /// The first existing anchor `candidate` collides with, as
    /// `(conflicting id, human-readable detail)`.
    fn first_conflict(&self, candidate: &Anchor) -> Option<(u32, String)> {
        self.anchors
            .iter()
            .find_map(|other| overlap_detail(other, candidate).map(|d| (other.id, d)))
    }

    /// The lowest id not already taken.
    fn next_id(&self) -> u32 {
        (0u32..).find(|id| self.get(*id).is_none()).unwrap_or(0)
    }
}

/// Great-circle distance between two WGS84 positions, in metres (haversine on a
/// sphere of radius [`EARTH_RADIUS`]).
///
/// `coordinate_system::transformation::geo_distance` does not fit here: it
/// returns a *pair* of axis-aligned component distances (north-south metres,
/// east-west metres at the mean latitude) for sizing a bounding box, not the
/// scalar centre-to-centre distance anchor separation is defined in.
pub fn great_circle_distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let d_phi = (lat2 - lat1).to_radians();
    let d_lambda = (lon2 - lon1).to_radians();

    let sin_half_phi = (d_phi / 2.0).sin();
    let sin_half_lambda = (d_lambda / 2.0).sin();
    let a =
        sin_half_phi * sin_half_phi + phi1.cos() * phi2.cos() * sin_half_lambda * sin_half_lambda;

    2.0 * EARTH_RADIUS * a.sqrt().atan2((1.0 - a).sqrt())
}

/// Euclidean distance between two Minecraft columns, in blocks.
fn mc_distance(x1: i32, z1: i32, x2: i32, z2: i32) -> f64 {
    let dx = f64::from(x2) - f64::from(x1);
    let dz = f64::from(z2) - f64::from(z1);
    dx.hypot(dz)
}

/// Snap an already-scaled Web Mercator coordinate (in blocks) onto the anchor
/// grid.
fn snap_to_grid(v: f64) -> i32 {
    ((v / ANCHOR_GRID_M).round() * ANCHOR_GRID_M) as i32
}

/// Reject an anchor that is malformed on its own terms.
fn validate_anchor(a: &Anchor) -> Result<(), String> {
    if !a.lat.is_finite() || !a.lon.is_finite() {
        return Err(format!(
            "anchor 'id {}' has a non-finite position ({}, {})",
            a.id, a.lat, a.lon
        ));
    }
    if a.lat.abs() > 90.0 {
        return Err(format!(
            "anchor 'id {}' has latitude {} outside +/-90 degrees",
            a.id, a.lat
        ));
    }
    if a.lon.abs() > 180.0 {
        return Err(format!(
            "anchor 'id {}' has longitude {} outside +/-180 degrees",
            a.id, a.lon
        ));
    }
    if !(a.radius_m.is_finite() && a.radius_m > 0.0) {
        return Err(format!(
            "anchor 'id {}' has a non-positive radius {}",
            a.id, a.radius_m
        ));
    }
    // The cap `AddAnchor` enforces, applied here so the handshake cannot bypass
    // it by declaring a continent-sized patch whose rim is grossly distorted.
    if a.radius_m > MAX_ANCHOR_RADIUS_M {
        return Err(format!(
            "anchor 'id {}' has radius {} m, above the maximum patch radius of {} m",
            a.id, a.radius_m, MAX_ANCHOR_RADIUS_M
        ));
    }
    if let Err(reason) = validate_scale(a.scale) {
        return Err(format!(
            "anchor 'id {}' has an unusable scale {}: {reason}",
            a.id, a.scale
        ));
    }
    Ok(())
}

/// Describe how `a` and `b` overlap, or `None` if they are far enough apart in
/// **both** geographic and Minecraft space.
///
/// Overlap is strict: centres exactly `r_a + r_b` apart are touching, not
/// overlapping, and are accepted.
///
/// The two checks are in different units and must not be mixed: the geographic
/// one compares metres of ground against the metre radii, the Minecraft one
/// compares blocks against the radii converted to blocks by each anchor's own
/// scale. At `scale == 1.0` the two sums coincide, which is why this used to
/// read as one number.
fn overlap_detail(a: &Anchor, b: &Anchor) -> Option<String> {
    let radii_sum_m = a.radius_m + b.radius_m;

    let geo = great_circle_distance_m(a.lat, a.lon, b.lat, b.lon);
    if geo < radii_sum_m {
        return Some(format!(
            "by {} (centres {} apart, radii sum {})",
            fmt_km(radii_sum_m - geo),
            fmt_km(geo),
            fmt_km(radii_sum_m)
        ));
    }

    let radii_sum_blocks = a.radius_blocks() + b.radius_blocks();
    let mc = mc_distance(a.mc_x, a.mc_z, b.mc_x, b.mc_z);
    if mc < radii_sum_blocks {
        return Some(format!(
            "by {} in Minecraft space (centres {} apart, radii sum {})",
            fmt_blocks(radii_sum_blocks - mc),
            fmt_blocks(mc),
            fmt_blocks(radii_sum_blocks)
        ));
    }

    None
}

/// Format a block distance for an error message. Blocks, not metres: the two
/// differ at any scale other than 1.0.
fn fmt_blocks(blocks: f64) -> String {
    if blocks.abs() < 10_000.0 {
        format!("{blocks:.0} blocks")
    } else {
        format!("{:.0}k blocks", blocks / 1000.0)
    }
}

/// Format a metre distance as kilometres for an error message.
fn fmt_km(m: f64) -> String {
    if m.abs() < 10_000.0 {
        format!("{:.1} km", m / 1000.0)
    } else {
        format!("{:.0} km", m / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MUNICH: (f64, f64) = (48.137, 11.575);
    const NEW_YORK: (f64, f64) = (40.713, -74.006);

    fn set_of(anchors: Vec<Anchor>) -> AnchorSet {
        AnchorSet::new(anchors).expect("fixture anchor set should validate")
    }

    #[test]
    fn test_projection_pins_anchor_to_its_minecraft_origin() {
        let a = Anchor::new(0, MUNICH.0, MUNICH.1, 1_300_000, -6_100_000, 500_000.0, 1.0);
        let (x, z) = a.projection().forward(a.lat, a.lon);
        assert!(
            (x - f64::from(a.mc_x)).abs() < 1e-6,
            "expected x == mc_x, got {x}"
        );
        assert!(
            (z - f64::from(a.mc_z)).abs() < 1e-6,
            "expected z == mc_z, got {z}"
        );
    }

    /// North is negative Z, and at the default scale a metre is a block.
    #[test]
    fn test_ten_km_north_is_ten_thousand_blocks_of_negative_z() {
        let a = Anchor::new(0, MUNICH.0, MUNICH.1, 0, 0, DEFAULT_ANCHOR_RADIUS_M, 1.0);
        let dlat = (10_000.0 / EARTH_RADIUS).to_degrees();
        let (x, z) = a.projection().forward(a.lat + dlat, a.lon);

        assert!(x.abs() < 1e-6, "due north should not move X, got {x}");
        assert!(
            (z + 10_000.0).abs() < 5.0,
            "10 km north should be z ~= -10000, got {z}"
        );
    }

    #[test]
    fn test_great_circle_distance_munich_to_new_york() {
        let d = great_circle_distance_m(MUNICH.0, MUNICH.1, NEW_YORK.0, NEW_YORK.1);
        assert!(
            (d - 6_488_000.0).abs() < 10_000.0,
            "Munich-New York should be ~6488 km, got {} km",
            d / 1000.0
        );
        assert_eq!(great_circle_distance_m(1.0, 2.0, 1.0, 2.0), 0.0);
    }

    #[test]
    fn test_munich_and_new_york_are_placed_far_apart_and_in_the_right_order() {
        let empty = AnchorSet::default();
        let munich = empty
            .place_new(MUNICH.0, MUNICH.1, DEFAULT_ANCHOR_RADIUS_M, 1.0)
            .expect("Munich should place into an empty set");
        assert_eq!(munich.id, 0);

        let mut set = AnchorSet::default();
        set.insert(munich).expect("Munich should insert");

        let ny = set
            .place_new(NEW_YORK.0, NEW_YORK.1, DEFAULT_ANCHOR_RADIUS_M, 1.0)
            .expect("New York should not conflict with Munich");
        assert_eq!(ny.id, 1);
        set.insert(ny).expect("New York should insert");

        // New York is west of Munich, so its X is smaller.
        assert!(
            ny.mc_x < munich.mc_x,
            "New York (x={}) should be west of Munich (x={})",
            ny.mc_x,
            munich.mc_x
        );

        // ...by a plausible amount: thousands of kilometres, not tens or
        // tens of thousands.
        let dx = f64::from(munich.mc_x) - f64::from(ny.mc_x);
        assert!(
            (5.0e6..2.0e7).contains(&dx),
            "east-west separation should be millions of blocks, got {dx}"
        );

        // Both snapped onto the anchor grid.
        for a in set.anchors() {
            assert_eq!(a.mc_x % ANCHOR_GRID_M as i32, 0, "x off grid: {}", a.mc_x);
            assert_eq!(a.mc_z % ANCHOR_GRID_M as i32, 0, "z off grid: {}", a.mc_z);
        }
    }

    #[test]
    fn test_anchors_100_km_apart_with_500_km_radii_are_rejected() {
        let mut set = AnchorSet::default();
        let first = set
            .place_new(48.0, 11.0, DEFAULT_ANCHOR_RADIUS_M, 1.0)
            .expect("first anchor should place");
        set.insert(first).expect("first anchor should insert");

        // 100 km due north.
        let dlat = (100_000.0 / EARTH_RADIUS).to_degrees();
        let err = set
            .place_new(48.0 + dlat, 11.0, DEFAULT_ANCHOR_RADIUS_M, 1.0)
            .expect_err("100 km apart with 500 km radii must be rejected");

        assert!(
            err.contains("id 0"),
            "error should name the conflict: {err}"
        );
        assert!(err.contains("overlap"), "error should say overlap: {err}");
        assert!(
            err.contains("900 km"),
            "error should quantify the overlap (1000 km sum - 100 km apart): {err}"
        );
        assert!(
            err.contains("radii sum 1000 km"),
            "error should state the radii sum: {err}"
        );
    }

    /// Documented boundary rule: touching is allowed, overlap is strict.
    ///
    /// Built in Minecraft space so the arithmetic is exact — integer block
    /// coordinates, no floating-point wobble on the decisive comparison. The
    /// two anchors are 1381 km apart geographically, so only the Minecraft
    /// check is at its boundary.
    #[test]
    fn test_exactly_touching_is_allowed_and_one_block_closer_is_not() {
        let touching = vec![
            Anchor::new(0, 60.0, 0.0, 0, 0, 500_000.0, 1.0),
            Anchor::new(1, 60.0, 25.0, 1_000_000, 0, 500_000.0, 1.0),
        ];
        assert!(
            AnchorSet::new(touching).is_ok(),
            "centres exactly r1 + r2 apart should be accepted"
        );

        let overlapping = vec![
            Anchor::new(0, 60.0, 0.0, 0, 0, 500_000.0, 1.0),
            Anchor::new(1, 60.0, 25.0, 999_999, 0, 500_000.0, 1.0),
        ];
        let err = AnchorSet::new(overlapping)
            .expect_err("one block closer than the radii sum must be rejected");
        assert!(
            err.contains("Minecraft space"),
            "the Minecraft-space check should be the one that fires: {err}"
        );
    }

    /// The same boundary in geographic space, with a metre of slack either side
    /// (an exact great-circle equality is not representable in f64).
    #[test]
    fn test_geographic_boundary_is_consistent_within_a_metre() {
        let base = Anchor::new(0, 0.0, 0.0, 0, 0, 500_000.0, 1.0);
        // Along the equator, great-circle distance is R * dlon.
        let lon_of = |d: f64| (d / EARTH_RADIUS).to_degrees();

        // Minecraft positions are put far apart so only the geographic check
        // can fire.
        let outside = Anchor::new(1, 0.0, lon_of(1_000_001.0), 5_000_000, 0, 500_000.0, 1.0);
        assert!(
            AnchorSet::new(vec![base, outside]).is_ok(),
            "a metre beyond the radii sum must be accepted"
        );

        let inside = Anchor::new(1, 0.0, lon_of(999_000.0), 5_000_000, 0, 500_000.0, 1.0);
        let err = AnchorSet::new(vec![base, inside])
            .expect_err("a kilometre inside the radii sum must be rejected");
        assert!(
            !err.contains("Minecraft space"),
            "the geographic check should be the one that fires: {err}"
        );
    }

    /// The case that justifies the second check: patches that are nowhere near
    /// each other on Earth, but land on top of each other in the world.
    ///
    /// Here it comes from client-supplied `mcX`/`mcZ` in the handshake — the
    /// mod's world may have been built with any placement at all.
    #[test]
    fn test_minecraft_overlap_without_geographic_overlap_is_rejected() {
        let a = Anchor::new(0, 0.0, 0.0, 0, 0, 500_000.0, 1.0);
        // 20 degrees of longitude at the equator is ~2224 km: no geographic
        // overlap at all with a 1000 km radii sum...
        let b = Anchor::new(1, 0.0, 20.0, 900_000, 0, 500_000.0, 1.0);
        assert!(
            great_circle_distance_m(a.lat, a.lon, b.lat, b.lon) > 2_000_000.0,
            "fixture should be geographically well separated"
        );
        // ...but only 900 km apart in blocks.
        let err = AnchorSet::new(vec![a, b]).expect_err("Minecraft overlap must be rejected");
        assert!(
            err.contains("Minecraft space"),
            "error should say which space overlapped: {err}"
        );
        assert!(
            err.contains("id 0"),
            "error should name the conflict: {err}"
        );
    }

    /// The same failure, reached the other way: through `place_new`, where grid
    /// snapping alone pulls two Web Mercator placements closer than the ground
    /// truth. Near the equator Web Mercator is true to scale, so snapping is
    /// the only distortion left — and it is enough.
    #[test]
    fn test_grid_snapping_can_create_a_minecraft_only_overlap() {
        let plane = WebMercatorProjection::new(0.0, 0.0, 1.0);
        // Chosen so both placements snap inward by nearly half a grid cell on
        // both axes: (-49999, -49999) -> (0, 0) and (949999, 149999) ->
        // (900000, 100000).
        let (lat_a, lon_a) = plane.inverse(-49_999.0, -49_999.0);
        let (lat_b, lon_b) = plane.inverse(949_999.0, 149_999.0);

        let mut set = AnchorSet::default();
        let a = set
            .place_new(lat_a, lon_a, 500_000.0, 1.0)
            .expect("first anchor should place");
        assert_eq!(
            (a.mc_x, a.mc_z),
            (0, 0),
            "fixture should snap to the origin"
        );
        set.insert(a).expect("first anchor should insert");

        // On the ground these centres are ~1020 km apart: more than the
        // 1000 km radii sum, so the geographic check is happy.
        let geo = great_circle_distance_m(lat_a, lon_a, lat_b, lon_b);
        assert!(
            geo > 1_000_000.0,
            "fixture must not overlap geographically, got {geo} m"
        );

        // Snapped, they are only ~906 km apart in blocks.
        let err = set
            .place_new(lat_b, lon_b, 500_000.0, 1.0)
            .expect_err("snapped-together patches must be rejected");
        assert!(
            err.contains("Minecraft space"),
            "the Minecraft-space check should be the one that fires: {err}"
        );
    }

    /// The mirror image, and why the geographic check is not redundant either:
    /// Web Mercator at 60 degrees N doubles distances, so two patches that
    /// genuinely overlap on the ground are placed 1700 km apart in blocks.
    #[test]
    fn test_geographic_overlap_without_minecraft_overlap_is_rejected() {
        let mut set = AnchorSet::default();
        let a = set
            .place_new(60.0, 0.0, 500_000.0, 1.0)
            .expect("first anchor should place");
        set.insert(a).expect("first anchor should insert");

        let b_probe = Anchor::new(99, 60.0, 15.0, 1_700_000, -8_400_000, 500_000.0, 1.0);
        assert!(
            mc_distance(a.mc_x, a.mc_z, b_probe.mc_x, b_probe.mc_z) > 1_000_000.0,
            "fixture should be well separated in Minecraft space"
        );

        let err = set
            .place_new(60.0, 15.0, 500_000.0, 1.0)
            .expect_err("832 km apart with 1000 km of radii must be rejected");
        assert!(
            !err.contains("Minecraft space"),
            "the geographic check should be the one that fires: {err}"
        );
        assert!(
            err.contains("id 0"),
            "error should name the conflict: {err}"
        );
    }

    #[test]
    fn test_find_containing_latlon() {
        let set = set_of(vec![
            Anchor::new(0, MUNICH.0, MUNICH.1, 1_300_000, -6_100_000, 500_000.0, 1.0),
            Anchor::new(
                1, NEW_YORK.0, NEW_YORK.1, -8_200_000, -5_000_000, 500_000.0, 1.0,
            ),
        ]);

        // Salzburg: ~130 km east of Munich, comfortably inside its patch.
        let found = set
            .find_containing_latlon(47.8095, 13.0550)
            .expect("Salzburg should fall inside the Munich patch");
        assert_eq!(found.id, 0);

        // Boston: ~300 km from New York, inside that patch instead.
        let found = set
            .find_containing_latlon(42.3601, -71.0589)
            .expect("Boston should fall inside the New York patch");
        assert_eq!(found.id, 1);

        // Tokyo is in neither.
        assert!(set.find_containing_latlon(35.6762, 139.6503).is_none());
    }

    #[test]
    fn test_find_containing_mc_and_contains_mc() {
        let a = Anchor::new(7, 0.0, 0.0, 1000, 2000, 500.0, 1.0);
        assert!(a.contains_mc(1200, 2000));
        assert!(!a.contains_mc(1600, 2000));
        // The rim is exclusive.
        assert!(!a.contains_mc(1500, 2000));

        let set = set_of(vec![a]);
        assert_eq!(set.find_containing_mc(1000, 2100).map(|x| x.id), Some(7));
        assert!(set.find_containing_mc(100_000, 0).is_none());
    }

    #[test]
    fn test_place_new_assigns_the_lowest_unused_id() {
        // Small radii so placement never conflicts; the point here is ids.
        let set = set_of(vec![
            Anchor::new(0, 0.0, 0.0, 0, 0, 1000.0, 1.0),
            Anchor::new(1, 10.0, 10.0, 1_000_000, 1_000_000, 1000.0, 1.0),
        ]);
        let next = set
            .place_new(-20.0, -20.0, 1000.0, 1.0)
            .expect("a distant anchor should place");
        assert_eq!(next.id, 2);

        // ...and it fills a gap rather than always appending.
        let gapped = set_of(vec![
            Anchor::new(0, 0.0, 0.0, 0, 0, 1000.0, 1.0),
            Anchor::new(2, 10.0, 10.0, 1_000_000, 1_000_000, 1000.0, 1.0),
            Anchor::new(3, 20.0, 20.0, 2_000_000, 2_000_000, 1000.0, 1.0),
        ]);
        let filler = gapped
            .place_new(-20.0, -20.0, 1000.0, 1.0)
            .expect("a distant anchor should place");
        assert_eq!(filler.id, 1, "expected the gap at id 1 to be filled");
    }

    #[test]
    fn test_duplicate_ids_are_rejected() {
        let err = AnchorSet::new(vec![
            Anchor::new(4, 0.0, 0.0, 0, 0, 1000.0, 1.0),
            Anchor::new(4, 10.0, 10.0, 1_000_000, 1_000_000, 1000.0, 1.0),
        ])
        .expect_err("duplicate ids must be rejected");
        assert!(err.contains("duplicate"), "unexpected error: {err}");
        assert!(err.contains('4'), "error should name the id: {err}");

        let mut set = set_of(vec![Anchor::new(4, 0.0, 0.0, 0, 0, 1000.0, 1.0)]);
        assert!(set
            .insert(Anchor::new(
                4, 10.0, 10.0, 1_000_000, 1_000_000, 1000.0, 1.0
            ))
            .is_err());
    }

    #[test]
    fn test_malformed_anchors_are_rejected() {
        assert!(AnchorSet::new(vec![Anchor::new(0, 91.0, 0.0, 0, 0, 1000.0, 1.0)]).is_err());
        assert!(AnchorSet::new(vec![Anchor::new(0, 0.0, 181.0, 0, 0, 1000.0, 1.0)]).is_err());
        assert!(AnchorSet::new(vec![Anchor::new(0, 0.0, 0.0, 0, 0, 0.0, 1.0)]).is_err());
        assert!(AnchorSet::new(vec![Anchor::new(0, f64::NAN, 0.0, 0, 0, 1.0, 1.0)]).is_err());
    }

    #[test]
    fn test_place_new_rejects_positions_web_mercator_cannot_represent() {
        let set = AnchorSet::default();
        let err = set
            .place_new(86.0, 0.0, DEFAULT_ANCHOR_RADIUS_M, 1.0)
            .expect_err("beyond the Web Mercator cutoff must be rejected");
        assert!(err.contains("latitude"), "unexpected error: {err}");

        assert!(set.place_new(0.0, 181.0, 1000.0, 1.0).is_err());
        assert!(set.place_new(0.0, 0.0, -1.0, 1.0).is_err());
        assert!(set.place_new(f64::NAN, 0.0, 1000.0, 1.0).is_err());
    }

    /// Regression: `Anchor::projection` used to hardcode a scale of 1.0 while
    /// the generator sized every feature by `GenConfig.scale`, so at scale 2.0
    /// double-width roads were painted onto a 1:1 street grid.
    #[test]
    fn test_projection_uses_the_anchor_scale() {
        let a = Anchor::new(3, MUNICH.0, MUNICH.1, 1_000_000, -2_000_000, 400_000.0, 2.0);
        let p = a.projection();
        assert_eq!(p.scale, 2.0, "the anchor scale must reach the projection");

        // The centre is still pinned exactly, at any scale.
        let (x, z) = p.forward(a.lat, a.lon);
        assert!(
            (x - 1_000_000.0).abs() < 1e-6,
            "expected x == mc_x, got {x}"
        );
        assert!(
            (z + 2_000_000.0).abs() < 1e-6,
            "expected z == mc_z, got {z}"
        );

        // 10 km due north is 20 000 blocks of negative Z at 2 blocks per metre.
        let dlat = (10_000.0 / EARTH_RADIUS).to_degrees();
        let (x, z) = p.forward(a.lat + dlat, a.lon);
        assert!(
            (x - 1_000_000.0).abs() < 1e-6,
            "due north should not move X, got {x}"
        );
        assert!(
            (z - (-2_000_000.0 - 20_000.0)).abs() < 10.0,
            "10 km north at scale 2 should be 20 000 blocks north of mc_z, got {z}"
        );

        // ...and the scaled position round-trips back to where it came from.
        let (lat, lon) = p.inverse(x, z);
        assert!(
            (lat - (a.lat + dlat)).abs() < 1e-7,
            "latitude should round-trip, got {lat}"
        );
        assert!(
            (lon - a.lon).abs() < 1e-7,
            "longitude should round-trip, got {lon}"
        );
    }

    /// `radius_m` is metres and `mc_x`/`mc_z` are blocks: at scale 2.0 a 100 km
    /// patch is 200 000 blocks wide but still only 100 km of ground.
    #[test]
    fn test_patch_extent_is_metres_geographically_and_blocks_in_the_world() {
        let scaled = Anchor::new(0, 0.0, 0.0, 0, 0, 100_000.0, 2.0);
        assert_eq!(scaled.radius_blocks(), 200_000.0);
        assert!(
            scaled.contains_mc(150_000, 0),
            "150 000 blocks is inside a 200 000-block patch"
        );
        assert!(!scaled.contains_mc(250_000, 0));

        // The geographic side is untouched by scale: 150 km of ground is still
        // outside a 100 km patch.
        let dlat = (150_000.0 / EARTH_RADIUS).to_degrees();
        assert!(!scaled.contains_latlon(dlat, 0.0));

        // At 1:1 the same block column is outside, which is what it used to be
        // for every scale.
        let unscaled = Anchor::new(0, 0.0, 0.0, 0, 0, 100_000.0, 1.0);
        assert_eq!(unscaled.radius_blocks(), 100_000.0);
        assert!(!unscaled.contains_mc(150_000, 0));
    }

    /// The Minecraft-space overlap check compares blocks with blocks, so the
    /// same pair of patches can be clear at 1:1 and overlapping at 2:1.
    #[test]
    fn test_minecraft_overlap_is_measured_in_blocks() {
        // 40 degrees of longitude at the equator is ~4450 km, so the geographic
        // check never fires here.
        let far = vec![
            Anchor::new(0, 0.0, 0.0, 0, 0, 500_000.0, 1.0),
            Anchor::new(1, 0.0, 40.0, 1_200_000, 0, 500_000.0, 1.0),
        ];
        assert!(
            AnchorSet::new(far).is_ok(),
            "1 200 000 blocks apart with 1 000 000 blocks of radii is clear"
        );

        let scaled = vec![
            Anchor::new(0, 0.0, 0.0, 0, 0, 500_000.0, 2.0),
            Anchor::new(1, 0.0, 40.0, 1_200_000, 0, 500_000.0, 2.0),
        ];
        let err = AnchorSet::new(scaled)
            .expect_err("at 2 blocks per metre those patches are 2 000 000 blocks wide");
        assert!(
            err.contains("Minecraft space"),
            "the Minecraft-space check should be the one that fires: {err}"
        );
        assert!(
            err.contains("blocks"),
            "the Minecraft-space message should be in blocks: {err}"
        );
    }

    /// Placement scales with the session too, so patches keep their relative
    /// geography and their clearances at any scale.
    #[test]
    fn test_place_new_scales_the_placement_plane() {
        let set = AnchorSet::default();
        let one = set
            .place_new(48.0, 11.0, 200_000.0, 1.0)
            .expect("1:1 placement should succeed");
        let two = set
            .place_new(48.0, 11.0, 200_000.0, 2.0)
            .expect("2:1 placement should succeed");

        assert_eq!(two.scale, 2.0, "the placed anchor should carry the scale");
        assert!(
            (f64::from(two.mc_x) - 2.0 * f64::from(one.mc_x)).abs() <= ANCHOR_GRID_M,
            "x should double (within one grid cell): {} vs {}",
            two.mc_x,
            one.mc_x
        );
        assert!(
            (f64::from(two.mc_z) - 2.0 * f64::from(one.mc_z)).abs() <= ANCHOR_GRID_M,
            "z should double (within one grid cell): {} vs {}",
            two.mc_z,
            one.mc_z
        );

        // Still on the grid, and still pinned to its own centre.
        assert_eq!(two.mc_x % ANCHOR_GRID_M as i32, 0);
        assert_eq!(two.mc_z % ANCHOR_GRID_M as i32, 0);
        let (x, z) = two.projection().forward(two.lat, two.lon);
        assert!((x - f64::from(two.mc_x)).abs() < 1e-6);
        assert!((z - f64::from(two.mc_z)).abs() < 1e-6);
    }

    /// Regression: `Hello`-supplied anchors used to bypass the 500 km cap that
    /// `AddAnchor` enforces, giving a patch whose rim is grossly distorted.
    #[test]
    fn test_handshake_anchors_are_capped_at_the_maximum_patch_radius() {
        let err = AnchorSet::new(vec![Anchor::new(7, 48.86, 2.35, 0, 0, 5_000_000.0, 1.0)])
            .expect_err("a 5000 km handshake patch must be rejected");
        assert!(err.contains("id 7"), "error should name the anchor: {err}");
        assert!(
            err.contains("5000000"),
            "error should quote the requested radius: {err}"
        );
        assert!(
            err.contains(&format!("{MAX_ANCHOR_RADIUS_M}")),
            "error should quote the cap: {err}"
        );

        // The cap itself is allowed, and it is what the default asks for.
        assert!(
            AnchorSet::new(vec![Anchor::new(
                7,
                48.86,
                2.35,
                0,
                0,
                MAX_ANCHOR_RADIUS_M,
                1.0
            )])
            .is_ok(),
            "exactly the cap must still be accepted"
        );

        // The same bound applies to the incremental paths.
        let mut set = AnchorSet::default();
        assert!(set
            .insert(Anchor::new(0, 0.0, 0.0, 0, 0, 900_000.0, 1.0))
            .is_err());
        let err = set
            .place_new(0.0, 0.0, 900_000.0, 1.0)
            .expect_err("placement must apply the same cap");
        assert!(
            err.contains(&format!("{MAX_ANCHOR_RADIUS_M}")),
            "error should quote the cap: {err}"
        );
    }

    /// A radius that is not a positive, finite number is rejected outright.
    #[test]
    fn test_non_finite_and_non_positive_radii_are_rejected() {
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            assert!(
                AnchorSet::new(vec![Anchor::new(0, 0.0, 0.0, 0, 0, bad, 1.0)]).is_err(),
                "radius {bad} should be rejected"
            );
            assert!(
                AnchorSet::default().place_new(0.0, 0.0, bad, 1.0).is_err(),
                "radius {bad} should be rejected at placement"
            );
        }
    }

    /// A scale the generation pipeline cannot use is rejected with the anchor,
    /// rather than reaching the projection.
    #[test]
    fn test_unusable_scales_are_rejected() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, 100.0] {
            assert!(
                AnchorSet::new(vec![Anchor::new(0, 0.0, 0.0, 0, 0, 1000.0, bad)]).is_err(),
                "scale {bad} should be rejected"
            );
            assert!(
                AnchorSet::default()
                    .place_new(0.0, 0.0, 1000.0, bad)
                    .is_err(),
                "scale {bad} should be rejected at placement"
            );
        }
    }

    #[test]
    fn test_empty_set_accessors() {
        let set = AnchorSet::default();
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert!(set.get(0).is_none());
        assert!(set.find_containing_latlon(0.0, 0.0).is_none());
        assert!(set.find_containing_mc(0, 0).is_none());
    }
}
