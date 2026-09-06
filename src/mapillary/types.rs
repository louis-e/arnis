//! Shared types for the facade pipeline. Port of `tools/facade_lab/common.py`.
//!
//! Every other module of the port depends on this one and on nothing else of the
//! port, the way the Python modules import only from `common.py`. The
//! conventions fixed here are the ones the whole pipeline assumes:
//!
//! * **Run frame** ([`Frame`]): equirectangular ENU metres about the bbox
//!   centre, x east, y north, z up. OSM rings are stored counter-clockwise
//!   (positive shoelace area), and for a CCW ring the outward normal of an edge
//!   with unit tangent `t` is `n = (t_y, -t_x)`. The mirrored rule in
//!   `facade.rs` is not a contradiction: that one is in the Arnis world frame,
//!   whose z axis points south.
//! * **Cluster z datum** is per cluster and drifts. [`Camera::ground_z`] and
//!   cluster point z values are comparable only inside one cluster; anything
//!   compared across views goes through wall coordinates `(s, h)` with a per
//!   view `z_base`.
//! * **Camera**: the rows of [`Camera::axes`] are the camera right, down and
//!   forward directions in ENU, and [`Camera::centre`] is the run frame centre.
//! * **Keys**: building `w<way id>` or `r<relation id>`; wall `<bkey>_<idx>`
//!   with a `p<k>` suffix when the wall was split; view `<wall key>__<pano id>`.
//!
//! Field names follow the Python ones so the two can be read side by side. The
//! exceptions are `Camera.C`, which is `centre` here, and the string valued
//! fields, which are enums: they carry the same wire strings through `as_str`
//! and `from_str`, so the JSON on disk is unchanged.

#![allow(dead_code)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Mean earth radius, the same value `coordinate_system::transformation` uses.
pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

// Occlusion bit field, one bit per reason a texel could not be read.
pub const OCC_FOOTPRINT: u8 = 1;
pub const OCC_CLOUD: u8 = 2;
pub const OCC_NADIR: u8 = 4;
pub const OCC_ZENITH: u8 = 8;
pub const OCC_SEG: u8 = 16;
pub const OCC_SKY: u8 = 32;
/// Perspective cameras only: the texel projects outside the image.
pub const OCC_OUTSIDE: u8 = 64;

// Block classes, packed into the alpha channel of `<wall key>.png`. The same
// values `facades::Class::from_alpha` decodes on the consumer side.
pub const CLS_WALL: u8 = 255;
pub const CLS_WINDOW: u8 = 192;
pub const CLS_DOOR: u8 = 128;
pub const CLS_UNKNOWN: u8 = 64;
pub const CLS_NODATA: u8 = 0;

// --------------------------------------------------------------------------- small enums

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }, default $default:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            /// The variant for a wire string, or the default when it is unknown.
            pub fn from_str_or_default(s: &str) -> Self {
                match s { $($text => Self::$variant,)+ _ => Self::$default }
            }
        }

        impl Default for $name {
            fn default() -> Self { Self::$default }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Ok(Self::from_str_or_default(&s))
            }
        }
    };
}

string_enum!(
    /// The OpenSfM camera model. `equirectangular` is Mapillary's other spelling
    /// of `spherical` and means the same projection.
    CameraModel {
        Spherical => "spherical",
        Perspective => "perspective",
        Brown => "brown",
        Fisheye => "fisheye",
    },
    default Spherical
);

impl CameraModel {
    /// True for the two spellings of the equirectangular model.
    pub fn is_spherical(self) -> bool {
        matches!(self, CameraModel::Spherical)
    }

    /// Parse, treating `equirectangular` as `spherical`.
    pub fn parse(s: &str) -> Self {
        match s {
            "equirectangular" => CameraModel::Spherical,
            other => CameraModel::from_str_or_default(other),
        }
    }
}

string_enum!(
    /// Where a camera's orientation came from, best first. The pipeline only
    /// ever produces `sfm` on the Munich fixture; the rest are the fallbacks.
    PoseSource {
        Sfm => "sfm",
        Sequence => "sequence",
        Autolevel => "autolevel",
        Heading => "heading",
    },
    default Heading
);

impl PoseSource {
    /// The confidence factor this pose source contributes.
    pub fn factor(self) -> f64 {
        match self {
            PoseSource::Sfm => 1.0,
            PoseSource::Sequence => 0.8,
            PoseSource::Autolevel => 0.65,
            PoseSource::Heading => 0.4,
        }
    }
}

string_enum!(
    /// Outcome of registering a cluster's point cloud against the footprints.
    RegSource {
        Local => "local",
        Global => "global",
        Unregistered => "none",
        NoCluster => "no_cluster",
    },
    default NoCluster
);

string_enum!(
    /// Where the wall plane came from.
    PlaneSource {
        Cloud => "cloud",
        OsmRegistered => "osm-registered",
        OsmRaw => "osm-raw",
    },
    default OsmRaw
);

string_enum!(
    /// Where the ground level under a camera came from.
    GroundSource {
        Cloud => "cloud",
        Sequence => "sequence",
        Default => "default",
    },
    default Default
);

string_enum!(
    /// Where an OSM height came from. `Default` means no usable tag and the
    /// 9 m fallback applies.
    HeightSource {
        Tag => "tag",
        Levels => "levels",
        Default => "default",
    },
    default Default
);

string_enum!(
    /// Whether a camera position is the SfM one or the raw GPS fix.
    GeometrySource {
        Computed => "computed",
        Gps => "gps",
    },
    default Computed
);

string_enum!(
    /// The OSM object a footprint came from.
    OsmKind {
        Way => "way",
        Relation => "relation",
    },
    default Way
);

string_enum!(
    /// Every gate a view candidate passes through, geometric first then image
    /// based. `outward` through `zenith` are decided on the pose alone, before
    /// any pixel is downloaded; the rest need the image.
    GateName {
        Outward => "outward",
        Near => "near",
        Far => "far",
        Incidence => "incidence",
        Angwidth => "angwidth",
        Fov => "fov",
        Nadir => "nadir",
        Zenith => "zenith",
        Los => "los",
        Blur => "blur",
        BlurRel => "blur_rel",
        Night => "night",
        Exposure => "exposure",
        Clipped => "clipped",
        Quality => "quality",
        Occlusion => "occlusion",
        Positions => "positions",
    },
    default Outward
);

/// Tier of a finished wall: A is texture plus blocks, B blocks only, C the
/// colour only, D nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    A,
    B,
    C,
    D,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::A => "A",
            Tier::B => "B",
            Tier::C => "C",
            Tier::D => "D",
        }
    }

    /// The tier for a wire string. Anything unrecognised is D, the tier that
    /// exports nothing, so a damaged record can never promote a wall.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "A" => Tier::A,
            "B" => Tier::B,
            "C" => Tier::C,
            _ => Tier::D,
        }
    }
}

// --------------------------------------------------------------------------- frame

/// Equirectangular ENU frame about the bbox centre.
///
/// `x = (lon - lon0) * pi/180 * R * cos(lat0)`, `y = (lat - lat0) * pi/180 * R`
/// with R = 6 371 000 m, which is under a centimetre of error at a kilometre.
/// OSM nodes and SfM clusters both reach this frame through lon/lat, clusters
/// via the WGS84 topocentric conversion in `geometry.rs`, so the sphere
/// approximation is applied to everything alike and cancels in relative
/// geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub lon0: f64,
    pub lat0: f64,
    kx: f64,
    ky: f64,
}

impl Frame {
    pub fn new(lon0: f64, lat0: f64) -> Self {
        let ky = std::f64::consts::PI / 180.0 * EARTH_RADIUS_M;
        Self {
            lon0,
            lat0,
            kx: ky * lat0.to_radians().cos(),
            ky,
        }
    }

    /// lon/lat in degrees to metres east/north.
    pub fn to_enu(self, lon: f64, lat: f64) -> [f64; 2] {
        [(lon - self.lon0) * self.kx, (lat - self.lat0) * self.ky]
    }

    /// Exact inverse of [`Frame::to_enu`].
    pub fn to_lonlat(self, xy: [f64; 2]) -> [f64; 2] {
        [xy[0] / self.kx + self.lon0, xy[1] / self.ky + self.lat0]
    }
}

/// A lat/lon bounding box in the order prefetch records it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
}

impl BBox {
    pub fn new(min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> Self {
        Self {
            min_lat,
            min_lon,
            max_lat,
            max_lon,
        }
    }

    pub fn centre(&self) -> (f64, f64) {
        (
            0.5 * (self.min_lon + self.max_lon),
            0.5 * (self.min_lat + self.max_lat),
        )
    }
}

// --------------------------------------------------------------------------- keys

/// `w<id>` for ways, `r<id>` for relations.
pub fn building_key(kind: OsmKind, osm_id: i64) -> String {
    match kind {
        OsmKind::Way => format!("w{osm_id}"),
        OsmKind::Relation => format!("r{osm_id}"),
    }
}

/// `<bkey>_<idx>`, plus `p<k>` when the wall was split into pieces.
pub fn wall_key(bkey: &str, idx: usize, piece: usize, n_pieces: usize) -> String {
    if n_pieces > 1 {
        format!("{bkey}_{idx}p{piece}")
    } else {
        format!("{bkey}_{idx}")
    }
}

pub fn view_key(wall_key: &str, pano_id: &str) -> String {
    format!("{wall_key}__{pano_id}")
}

/// Splits a view key back into (wall key, pano id) at the last `__`.
pub fn split_view_key(view_key: &str) -> Option<(&str, &str)> {
    view_key
        .rfind("__")
        .map(|i| (&view_key[..i], &view_key[i + 2..]))
}

// --------------------------------------------------------------------------- Params

/// Every tunable in one place, mirroring `common.Params` field for field.
///
/// Constructed once per run and passed down; nothing reads a global. The
/// defaults are the values of the reference run, and
/// `tests/golden/facade/params.json` holds the same table, so a threshold that
/// drifts during the port fails a test rather than a facade.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Params {
    // geometry (geo.py)
    pub min_wall_m: f64,
    pub split_wall_m: f64,
    pub merge_deg: f64,
    pub min_ring_area_m2: f64,
    pub metres_per_level: f64,
    pub default_height_m: f64,
    /// Texel offset off the wall plane, so a texel is never exactly on it.
    pub eps_m: f64,
    /// Fetch margin for imagery, and the wider one for OSM: a building outside
    /// the box can still occlude one inside it.
    pub pano_margin_m: f64,
    pub osm_margin_m: f64,
    // visibility
    pub view_dist: [f64; 2],
    pub far_dist_m: f64,
    pub max_incidence_deg: f64,
    pub min_angwidth_deg: f64,
    pub los_samples: u32,
    pub los_min_visible: f64,
    /// Panoramas only: the nadir and zenith band of the rig, where the vehicle
    /// and the mount are in the picture.
    pub v_range: [f64; 2],
    /// Perspective: the share of the wall rectangle (11 x 5 samples) that must
    /// land on the image.
    pub fov_min_on_image: f64,
    /// Samples within this share of the border do not count, which guards
    /// against a wall that is only just cut off.
    pub persp_margin: f64,
    /// Portrait phone shots are dropped: their orientation is unreliable.
    pub persp_landscape_only: bool,
    /// Score factor for perspective (phone, dashcam) candidates. 1.0 is no
    /// preference and 0 is a hard rule. Measured on the Munich box: the
    /// preference buys nothing and costs a little, because 22 of the 23 walls
    /// whose best view is a phone frame have no panorama that passes the gates
    /// at all, so no factor can reach them. Kept because it pins the behaviour
    /// in a test. See MEASURED.md.
    pub perspective_penalty: f64,
    /// Camera classes the geometry stage admits. Perspective imagery is on by
    /// default as extra views: on the Munich box it raised the tier A count
    /// from 90 to 99 walls, with the coverage caps keeping the partial walls
    /// out.
    pub camera_types: Vec<CameraModel>,
    // pose (pose.py)
    pub seq_window_s: f64,
    pub seq_min_panos: u32,
    pub roll_max_deg: f64,
    pub roll_sd_floor_deg: f64,
    pub roll_sd_mult: f64,
    pub autolevel_size: u32,
    pub autolevel_min_lines: u32,
    pub autolevel_max_residual_deg: f64,
    pub autolevel_line_deg: f64,
    // sfm (sfm.py)
    pub ground_r: [f64; 2],
    pub ground_min_pts: u32,
    pub ground_pct: f64,
    /// Panorama rigs: a car roof or a backpack.
    pub cam_height_range: [f64; 2],
    pub rig_height_default_m: f64,
    /// Phones, dashcams and helmet cameras.
    pub persp_cam_height_range: [f64; 2],
    /// Measured on the Munich phones and dashcams, which sat at 1.2 to 1.7 m.
    pub persp_height_default_m: f64,
    pub foot_search_m: f64,
    pub foot_min_pts: u32,
    pub foot_pct: f64,
    pub depth_radius_m: f64,
    pub depth_size: [u32; 2],
    pub shot_time_tol_s: f64,
    pub metric_tol: f64,
    // registration
    pub reg_radius_m: f64,
    pub reg_check_radius_m: f64,
    pub reg_coarse_m: f64,
    pub reg_fine_m: f64,
    pub reg_step_fine: f64,
    pub reg_min_inliers: f64,
    pub reg_uniqueness: f64,
    pub reg_theta_deg: f64,
    pub reg_theta_step: f64,
    pub reg_band: [f64; 2],
    // plane
    pub plane_band_m: f64,
    pub plane_angle_step_deg: f64,
    pub plane_search_m: f64,
    pub plane_max_angle_deg: f64,
    pub plane_z_range: [f64; 2],
    // rectify
    pub loose_ppm: [f64; 2],
    pub tex_ppb: u32,
    pub top_margin: [f64; 2],
    /// Height of the rectangle above a default height wall: max(2.2h, h+12, 30).
    pub top_margin_default_m: f64,
    // refine
    pub lean_max_deg: f64,
    pub lean_max_keystone: f64,
    pub lean_min_lines: u32,
    pub lean_min_inliers: f64,
    pub plane_gate_deg_per_m: f64,
    pub roof_ratio: [f64; 2],
    pub roof_spread_m: f64,
    pub extent_window_m: f64,
    pub extent_lambda: f64,
    pub phase_range_m: f64,
    pub phase_step_m: f64,
    // blocks
    pub dark_thr: f64,
    pub window_darkfrac: f64,
    pub door_band_m: f64,
    pub unknown_nodata: f64,
    pub palette_k: u32,
    pub palette_merge: f64,
    // tiers
    pub tier_a: f64,
    pub tier_b: f64,
    pub tier_c: f64,
    /// Coverage caps: tier A needs at least 60 per cent observed blocks and
    /// tier B at least 20 per cent; below that only the colour is exported.
    pub tier_a_max_unknown: f64,
    pub tier_b_max_unknown: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            min_wall_m: 3.0,
            split_wall_m: 30.0,
            merge_deg: 4.0,
            min_ring_area_m2: 4.0,
            metres_per_level: 3.0,
            default_height_m: 9.0,
            eps_m: 0.05,
            pano_margin_m: 45.0,
            osm_margin_m: 60.0,
            view_dist: [4.0, 35.0],
            far_dist_m: 45.0,
            max_incidence_deg: 60.0,
            min_angwidth_deg: 12.0,
            los_samples: 9,
            los_min_visible: 0.6,
            v_range: [0.05, 0.78],
            fov_min_on_image: 0.35,
            persp_margin: 0.06,
            persp_landscape_only: true,
            perspective_penalty: 1.0,
            camera_types: vec![
                CameraModel::Spherical,
                CameraModel::Perspective,
                CameraModel::Brown,
            ],
            seq_window_s: 60.0,
            seq_min_panos: 3,
            roll_max_deg: 20.0,
            roll_sd_floor_deg: 1.5,
            roll_sd_mult: 3.0,
            autolevel_size: 768,
            autolevel_min_lines: 15,
            autolevel_max_residual_deg: 1.5,
            autolevel_line_deg: 25.0,
            ground_r: [2.5, 8.0],
            ground_min_pts: 30,
            ground_pct: 5.0,
            cam_height_range: [1.2, 4.5],
            rig_height_default_m: 2.5,
            persp_cam_height_range: [0.8, 2.6],
            persp_height_default_m: 1.4,
            foot_search_m: 3.0,
            foot_min_pts: 20,
            foot_pct: 5.0,
            depth_radius_m: 60.0,
            depth_size: [720, 360],
            shot_time_tol_s: 1.0,
            metric_tol: 0.02,
            reg_radius_m: 40.0,
            reg_check_radius_m: 25.0,
            reg_coarse_m: 12.0,
            reg_fine_m: 1.5,
            reg_step_fine: 0.25,
            reg_min_inliers: 0.5,
            reg_uniqueness: 1.2,
            reg_theta_deg: 3.0,
            reg_theta_step: 0.5,
            reg_band: [3.0, 22.0],
            plane_band_m: 0.25,
            plane_angle_step_deg: 0.02,
            plane_search_m: 3.0,
            plane_max_angle_deg: 10.0,
            plane_z_range: [1.0, 30.0],
            loose_ppm: [6.0, 20.0],
            tex_ppb: 8,
            top_margin: [2.2, 12.0],
            top_margin_default_m: 30.0,
            lean_max_deg: 6.0,
            lean_max_keystone: 0.3,
            lean_min_lines: 15,
            lean_min_inliers: 0.6,
            plane_gate_deg_per_m: 0.35,
            roof_ratio: [0.5, 1.7],
            roof_spread_m: 2.0,
            extent_window_m: 3.0,
            extent_lambda: 1.0,
            phase_range_m: 0.5,
            phase_step_m: 0.125,
            dark_thr: 0.12,
            window_darkfrac: 0.4,
            door_band_m: 3.0,
            unknown_nodata: 0.3,
            palette_k: 3,
            palette_merge: 0.06,
            tier_a: 0.6,
            tier_b: 0.38,
            tier_c: 0.2,
            tier_a_max_unknown: 0.4,
            tier_b_max_unknown: 0.8,
        }
    }
}

impl Params {
    /// True when this run admits the given camera class.
    pub fn admits(&self, model: CameraModel) -> bool {
        let model = if model.is_spherical() {
            CameraModel::Spherical
        } else {
            model
        };
        self.camera_types.contains(&model)
    }

    /// The camera height prior for a class, used until the cloud says better.
    pub fn height_prior(&self, model: CameraModel) -> f64 {
        if model.is_spherical() {
            self.rig_height_default_m
        } else {
            self.persp_height_default_m
        }
    }

    /// Short stable digest of every value, for the on-disk cache key.
    ///
    /// Deliberately not the Python `Params.hash`: it only has to be stable and
    /// to change whenever any threshold changes, and the two caches are never
    /// read by each other.
    pub fn digest(&self) -> String {
        use std::hash::Hasher;
        let json = serde_json::to_string(self).unwrap_or_default();
        let mut h = fnv::FnvHasher::default();
        h.write(json.as_bytes());
        format!("{:016x}", h.finish())
    }
}

// --------------------------------------------------------------------------- geometry products

/// One footprint in the run frame: exterior ring CCW and not closed, holes for
/// the line of sight test.
///
/// `target` is true when the footprint touches the original unpadded bbox; the
/// fetch margins bring in neighbours that only ever serve as occluders.
#[derive(Clone, Debug)]
pub struct Building {
    pub key: String,
    pub osm_id: i64,
    pub kind: OsmKind,
    pub ring: Vec<[f64; 2]>,
    pub holes: Vec<Vec<[f64; 2]>>,
    /// Ring node ids, in the same order as `ring`.
    pub node_ids: Vec<i64>,
    pub tags: BTreeMap<String, String>,
    pub height_osm: Option<f64>,
    pub height_source: HeightSource,
    pub min_height: f64,
    pub target: bool,
    /// Way ids a relation owns, so they are not emitted as buildings of their
    /// own.
    pub member_ways: Vec<i64>,
}

impl Building {
    /// The height to build with, falling back to the default.
    pub fn height_m(&self, params: &Params) -> f64 {
        self.height_osm.unwrap_or(params.default_height_m)
    }

    pub fn centroid(&self) -> [f64; 2] {
        let n = self.ring.len().max(1) as f64;
        let mut c = [0.0, 0.0];
        for p in &self.ring {
            c[0] += p[0];
            c[1] += p[1];
        }
        [c[0] / n, c[1] / n]
    }
}

/// One original OSM ring edge inside a merged wall, with the interval it
/// covers along that wall, so the Arnis export can map blocks back to raw
/// edges and from there to node ids.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallEdge {
    pub edge_idx: usize,
    pub node_a: i64,
    pub node_b: i64,
    pub s0: f64,
    pub s1: f64,
}

/// A wall segment with its outward normal. [`Wall::point`] is the only place
/// wall coordinates become 3D.
#[derive(Clone, Debug)]
pub struct Wall {
    pub key: String,
    pub building_key: String,
    pub idx: usize,
    pub node_a: i64,
    pub node_b: i64,
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// Outward normal in plan, unit length.
    pub n: [f64; 2],
    pub length: f64,
    /// Ring edge indices this wall was merged from.
    pub merged_idx: Vec<usize>,
    pub piece: usize,
    pub n_pieces: usize,
    pub height_osm: Option<f64>,
    pub height_source: HeightSource,
    pub reachable: bool,
    pub unreachable_reason: String,
    pub edges: Vec<WallEdge>,
    /// s of this piece's `a` along the unsplit merged wall.
    pub s_offset: f64,
}

impl Wall {
    pub fn tangent(&self) -> [f64; 2] {
        let t = [self.b[0] - self.a[0], self.b[1] - self.a[1]];
        let len = (t[0] * t[0] + t[1] * t[1]).sqrt().max(1e-12);
        [t[0] / len, t[1] / len]
    }

    pub fn midpoint(&self) -> [f64; 2] {
        [0.5 * (self.a[0] + self.b[0]), 0.5 * (self.a[1] + self.b[1])]
    }

    /// The texel at wall coordinates `(s, h)`: `a + s t + eps n` in plan and
    /// `z_base + h` in height.
    pub fn point(&self, s: f64, h: f64, z_base: f64, eps: f64) -> [f64; 3] {
        let t = self.tangent();
        [
            self.a[0] + s * t[0] + eps * self.n[0],
            self.a[1] + s * t[1] + eps * self.n[1],
            z_base + h,
        ]
    }

    /// `(s, h, d)` of a 3D point: along the wall, above the base, and the
    /// signed distance in front of it, positive outward.
    pub fn sh_of(&self, p: [f64; 3], z_base: f64) -> (f64, f64, f64) {
        let t = self.tangent();
        let rel = [p[0] - self.a[0], p[1] - self.a[1]];
        (
            rel[0] * t[0] + rel[1] * t[1],
            p[2] - z_base,
            rel[0] * self.n[0] + rel[1] * self.n[1],
        )
    }

    pub fn height_m(&self, params: &Params) -> f64 {
        self.height_osm.unwrap_or(params.default_height_m)
    }
}

// --------------------------------------------------------------------------- imagery

/// One Graph API image record, typed.
///
/// `lon`/`lat` are `computed_geometry` when present, else `geometry`;
/// `compass` is `computed_compass_angle` else `compass_angle`; `alt` is
/// `computed_altitude`, which is metres in the cluster's own topocentric datum
/// and NOT the ellipsoidal `altitude` field.
#[derive(Clone, Debug)]
pub struct PanoMeta {
    pub id: String,
    pub lon: f64,
    pub lat: f64,
    pub alt: f64,
    pub compass: f64,
    pub rotation: Option<[f64; 3]>,
    pub atomic_scale: Option<f64>,
    pub captured_at: i64,
    pub sequence: String,
    pub quality: f64,
    pub width: u32,
    pub height: u32,
    pub cluster_id: Option<String>,
    pub geometry_source: GeometrySource,
    pub camera_type: CameraModel,
    pub camera_params: Vec<f64>,
}

impl PanoMeta {
    pub fn is_spherical(&self) -> bool {
        self.camera_type.is_spherical()
    }

    /// One Graph API image record. `None` when it carries no usable position.
    ///
    /// The `computed_*` fields are the SfM answers and are preferred wherever
    /// they exist; the plain ones are the raw GPS fix and the device compass.
    pub fn from_graph(d: &serde_json::Value) -> Option<Self> {
        let id = match d.get("id") {
            Some(v) if v.is_string() => v.as_str()?.to_string(),
            Some(v) => v.as_i64()?.to_string(),
            None => return None,
        };
        let geom = d
            .get("computed_geometry")
            .filter(|v| !v.is_null())
            .or_else(|| d.get("geometry"));
        let coords = geom
            .and_then(|g| g.get("coordinates"))
            .and_then(|c| c.as_array())?;
        let lon = coords.first()?.as_f64()?;
        let lat = coords.get(1)?.as_f64()?;
        let num = |key: &str| d.get(key).and_then(serde_json::Value::as_f64);
        let compass = num("computed_compass_angle")
            .or_else(|| num("compass_angle"))
            .unwrap_or(0.0)
            .rem_euclid(360.0);
        let vec3 = |key: &str| -> Option<[f64; 3]> {
            let a = d.get(key)?.as_array()?;
            Some([
                a.first()?.as_f64()?,
                a.get(1)?.as_f64()?,
                a.get(2)?.as_f64()?,
            ])
        };
        let camera_type = d
            .get("camera_type")
            .and_then(serde_json::Value::as_str)
            .map(CameraModel::parse)
            .unwrap_or(
                if d.get("is_pano").and_then(serde_json::Value::as_bool) == Some(true) {
                    CameraModel::Spherical
                } else {
                    CameraModel::Perspective
                },
            );
        Some(PanoMeta {
            id,
            lon,
            lat,
            alt: num("computed_altitude").unwrap_or(0.0),
            compass,
            rotation: vec3("computed_rotation"),
            atomic_scale: num("atomic_scale"),
            captured_at: d
                .get("captured_at")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
            sequence: d
                .get("sequence")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            quality: num("quality_score").unwrap_or(0.0),
            width: d
                .get("width")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32,
            height: d
                .get("height")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32,
            cluster_id: d
                .get("sfm_cluster")
                .and_then(|c| c.get("id"))
                .map(|v| match v.as_str() {
                    Some(s) => s.to_string(),
                    None => v.to_string(),
                }),
            geometry_source: if d.get("computed_geometry").is_some_and(|v| !v.is_null()) {
                GeometrySource::Computed
            } else {
                GeometrySource::Gps
            },
            camera_type,
            camera_params: d
                .get("camera_parameters")
                .and_then(serde_json::Value::as_array)
                .map(|a| a.iter().filter_map(serde_json::Value::as_f64).collect())
                .unwrap_or_default(),
        })
    }
}

/// Per pano registration outcome.
///
/// `(dx, dy, theta_deg)` maps raw cluster geometry onto OSM: a point `p`
/// becomes `Rz(theta) (p - C_raw) + C_raw + (dx, dy)`. OSM never moves.
#[derive(Clone, Copy, Debug, Default)]
pub struct RegResult {
    pub dx: f64,
    pub dy: f64,
    pub theta_deg: f64,
    pub inliers_before: f64,
    pub inliers_after: f64,
    pub ambiguity_ratio: f64,
    pub radius_agreement_m: f64,
    pub n_points: usize,
    pub accepted: bool,
    pub source: RegSource,
    /// Kept for the factor table; the `atomic_scale` gate never sets it.
    pub scale_suspect: bool,
}

impl RegResult {
    pub fn shift(&self) -> [f64; 3] {
        [self.dx, self.dy, self.theta_deg]
    }
}

/// Everything the projection needs for one image after the align stage.
///
/// `centre` is the run frame position: xy in ENU metres, z in this pano's
/// cluster datum. The rows of `axes` are the camera right, down and forward
/// directions in ENU. `ground_z` is the cluster-datum ground under the camera
/// and `cam_height_m = centre.z - ground_z`; only `cam_height_m` is comparable
/// across clusters. `centre` and `axes` already include the accepted
/// registration shift once `reg` is set.
#[derive(Clone, Debug)]
pub struct Camera {
    pub pano_id: String,
    pub centre: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub pose_source: PoseSource,
    pub roll_deg: f64,
    pub pitch_deg: f64,
    pub ground_z: f64,
    pub cam_height_m: f64,
    pub ground_source: GroundSource,
    pub cluster_id: Option<String>,
    pub shot_id: Option<String>,
    pub reg: Option<RegResult>,
    pub compass_deg: f64,
    pub pose_factor: f64,
    pub width: u32,
    pub height: u32,
    pub camera_type: CameraModel,
    pub camera_params: Vec<f64>,
}

impl Camera {
    pub fn is_spherical(&self) -> bool {
        self.camera_type.is_spherical()
    }
}

/// One gate a candidate went through, with the value the gate saw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gate {
    pub name: GateName,
    pub passed: bool,
    pub value: f64,
}

/// The result of the gates for one (wall, image) pair. Rejected candidates are
/// kept so a wall's absence can be explained.
#[derive(Clone, Debug)]
pub struct ViewCandidate {
    pub wall_key: String,
    pub pano_id: String,
    pub dist_m: f64,
    pub incidence_deg: f64,
    pub angwidth_deg: f64,
    pub visible_frac: f64,
    /// The visible interval along the wall, in metres from `a`.
    pub s_vis: [f64; 2],
    pub g_score: f64,
    pub gates: Vec<Gate>,
    pub f_occ: f64,
    pub blur: f64,
    pub score: f64,
    pub rejected_reason: Option<String>,
}

impl ViewCandidate {
    pub fn gate(&self, name: GateName) -> Option<Gate> {
        self.gates.iter().copied().find(|g| g.name == name)
    }

    pub fn set_gate(&mut self, name: GateName, passed: bool, value: f64) {
        let g = Gate {
            name,
            passed,
            value,
        };
        match self.gates.iter_mut().find(|g| g.name == name) {
            Some(slot) => *slot = g,
            None => self.gates.push(g),
        }
    }

    pub fn accepted(&self) -> bool {
        self.rejected_reason.is_none()
    }
}

/// The wall plane after the align stage: a line `n . xy = d` in plan with `n`
/// outward, plus the provisional ends projected onto it.
///
/// `z_top98` is in the cluster datum of the view whose registration produced
/// the fit.
#[derive(Clone, Debug)]
pub struct PlaneFit {
    pub wall_key: String,
    pub n: [f64; 2],
    pub d: f64,
    pub source: PlaneSource,
    pub n_inliers: usize,
    pub pts_per_m: f64,
    pub rms_m: f64,
    pub angle_vs_osm_deg: f64,
    pub offset_vs_osm_m: f64,
    pub z_top98: Option<f64>,
    pub z_continuous: bool,
    pub ambiguity: f64,
    pub a_ref: Option<[f64; 2]>,
    pub b_ref: Option<[f64; 2]>,
    pub pano_id: Option<String>,
    pub cluster_id: Option<String>,
}

// --------------------------------------------------------------------------- the wall product

/// The final facade rectangle in wall coordinates, with where each edge of it
/// came from.
#[derive(Clone, Debug)]
pub struct WallDecision {
    pub wall_key: String,
    pub s_l: f64,
    pub s_r: f64,
    pub src_a: String,
    pub src_b: String,
    pub trimmed_a: i32,
    pub trimmed_b: i32,
    pub h_used: f64,
    pub height_source: String,
    pub h_sky: Option<f64>,
    pub h_cloud: Option<f64>,
    pub h_osm: Option<f64>,
    pub phase: [f64; 2],
    pub bimodality: f64,
    pub flags: Vec<String>,
}

/// The Arnis facing product for one wall: the 1 m block grid with its classes,
/// the 8 px per metre texture, and everything the consumer needs to place it.
///
/// `rgb` and `cls` are row major with row 0 at the top of the wall and column 0
/// at the first OSM node of the edge, which is the layout `facades.rs` already
/// reads from the exported PNG.
#[derive(Clone, Debug)]
pub struct WallProduct {
    pub wall_key: String,
    pub building_key: String,
    pub node_a: i64,
    pub node_b: i64,
    /// Which piece of a split span this is, and how many there are. Part of the
    /// wall's identity: a span longer than `split_wall_m` is cut into pieces
    /// that all cover the same ring edges, so the node ids alone do not tell
    /// two pieces apart.
    pub piece: usize,
    pub n_pieces: usize,
    /// The original ring edges with the block column interval each one covers.
    pub edges: Vec<WallEdge>,
    /// Metres from the start of this piece to the left edge of column 0.
    pub col0_m: f64,
    pub cols: u32,
    pub rows: u32,
    pub rgb: Vec<[u8; 3]>,
    pub cls: Vec<u8>,
    /// True where a block was actually seen, as opposed to filled in.
    pub observed: Vec<bool>,
    /// Per row, the colour of the floor band there.
    pub bands: Vec<[u8; 3]>,
    /// 8 px per metre texture, RGBA with alpha as the valid mask.
    pub tex: Option<image::RgbaImage>,
    pub tier: Tier,
    pub confidence: f64,
    pub height_used_m: f64,
    pub unknown_share: f64,
    pub views: Vec<String>,
    pub flags: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips() {
        let f = Frame::new(11.5795305, 48.13643);
        let xy = f.to_enu(11.58, 48.137);
        let back = f.to_lonlat(xy);
        assert!((back[0] - 11.58).abs() < 1e-12);
        assert!((back[1] - 48.137).abs() < 1e-12);
        // A degree of latitude is about 111.2 km, and longitude is shortened by
        // cos(lat0) at this latitude.
        assert!((f.to_enu(11.5795305, 49.13643)[1] - 111_194.9).abs() < 1.0);
        assert!((f.to_enu(12.5795305, 48.13643)[0] - 74_240.0).abs() < 50.0);
    }

    #[test]
    fn keys_have_the_python_shape() {
        assert_eq!(building_key(OsmKind::Way, 81190157), "w81190157");
        assert_eq!(building_key(OsmKind::Relation, 147094), "r147094");
        assert_eq!(wall_key("w81190157", 2, 0, 1), "w81190157_2");
        assert_eq!(wall_key("w81190192", 0, 1, 2), "w81190192_0p1");
        let v = view_key("w81190157_2", "2124891284665287");
        assert_eq!(v, "w81190157_2__2124891284665287");
        assert_eq!(
            split_view_key(&v),
            Some(("w81190157_2", "2124891284665287"))
        );
    }

    #[test]
    fn wall_coordinates_are_consistent() {
        let w = Wall {
            key: "w1_0".into(),
            building_key: "w1".into(),
            idx: 0,
            node_a: 1,
            node_b: 2,
            a: [0.0, 0.0],
            b: [10.0, 0.0],
            n: [0.0, -1.0],
            length: 10.0,
            merged_idx: vec![0],
            piece: 0,
            n_pieces: 1,
            height_osm: Some(12.0),
            height_source: HeightSource::Tag,
            reachable: true,
            unreachable_reason: String::new(),
            edges: vec![],
            s_offset: 0.0,
        };
        let p = w.point(4.0, 3.0, 1.0, 0.05);
        assert!((p[0] - 4.0).abs() < 1e-12);
        assert!((p[1] + 0.05).abs() < 1e-12);
        assert!((p[2] - 4.0).abs() < 1e-12);
        let (s, h, d) = w.sh_of(p, 1.0);
        assert!((s - 4.0).abs() < 1e-12);
        assert!((h - 3.0).abs() < 1e-12);
        assert!((d - 0.05).abs() < 1e-12);
    }

    #[test]
    fn camera_types_gate_the_run() {
        let p = Params::default();
        assert!(p.admits(CameraModel::Spherical));
        assert!(p.admits(CameraModel::Perspective));
        assert!(!p.admits(CameraModel::Fisheye));
        let spherical_only = Params {
            camera_types: vec![CameraModel::Spherical],
            ..Params::default()
        };
        assert!(!spherical_only.admits(CameraModel::Perspective));
        assert!((p.height_prior(CameraModel::Spherical) - 2.5).abs() < 1e-12);
        assert!((p.height_prior(CameraModel::Brown) - 1.4).abs() < 1e-12);
    }
}
