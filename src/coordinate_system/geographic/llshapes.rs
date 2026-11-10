use super::{LLBBox, LLPoint};

/// Most points a set of shapes may have: a detailed city boundary has a few ten thousand.
pub const MAX_SHAPE_POINTS: usize = 200_000;

/// One polygon as `(lat, lng)` rings, outer ring first.
pub type LLRings = Vec<Vec<(f64, f64)>>;

/// Areas drawn on the map, merged into one: each polygon is an outer ring and any holes.
#[derive(Clone, Debug, PartialEq)]
pub struct LLShapes {
    polygons: Vec<Vec<Vec<LLPoint>>>,
}

impl LLShapes {
    /// Polygons of `(lat, lng)` rings, outer first; each ring needs three distinct points and area.
    pub fn new(polygons: Vec<LLRings>) -> Result<Self, String> {
        if polygons.is_empty() {
            return Err("No shape was given.".to_string());
        }
        let points: usize = polygons.iter().flatten().map(Vec::len).sum();
        if points > MAX_SHAPE_POINTS {
            return Err(format!(
                "The shapes have {points} points; at most {MAX_SHAPE_POINTS} are supported."
            ));
        }
        let mut out = Vec::with_capacity(polygons.len());
        for (i, rings) in polygons.into_iter().enumerate() {
            let n = i + 1;
            if rings.is_empty() {
                return Err(format!("Shape {n} has no outline."));
            }
            let mut checked = Vec::with_capacity(rings.len());
            for ring in rings {
                checked.push(check_ring(ring).map_err(|e| format!("Shape {n}: {e}"))?);
            }
            out.push(checked);
        }
        // Areas on both sides of the meridian would put the whole globe in between.
        let (_, w, _, e) = extent(out.iter().flatten().flatten());
        if e - w > 180.0 {
            return Err(format!("the shapes together: {ACROSS_MERIDIAN}"));
        }
        Ok(Self { polygons: out })
    }

    /// One `--polygon` value: `lat,lng` points separated by spaces or semicolons.
    pub fn parse_points(s: &str) -> Result<Vec<(f64, f64)>, String> {
        let mut points = Vec::new();
        for pair in s
            .split([' ', ';', '\n', '\t'])
            .filter(|p| !p.trim().is_empty())
        {
            let mut it = pair.split(',').map(str::trim);
            let (Some(lat), Some(lng), None) = (it.next(), it.next(), it.next()) else {
                return Err(format!("'{pair}' is not a lat,lng point"));
            };
            let num = |v: &str| {
                v.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("'{v}' in '{pair}' is not a number"))
            };
            points.push((num(lat)?, num(lng)?));
        }
        Ok(points)
    }

    /// The Polygon and MultiPolygon geometries of a GeoJSON document, holes included.
    pub fn from_geojson(text: &str) -> Result<Vec<LLRings>, String> {
        let doc: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("not valid GeoJSON: {e}"))?;
        let mut polygons = Vec::new();
        collect_geojson(&doc, &mut polygons, 0)?;
        if polygons.is_empty() {
            return Err("no Polygon or MultiPolygon geometry found".to_string());
        }
        Ok(polygons)
    }

    pub fn polygons(&self) -> &[Vec<Vec<LLPoint>>] {
        &self.polygons
    }

    /// The rectangle around every shape: what is downloaded and processed.
    pub fn bbox(&self) -> LLBBox {
        let (s, w, n, e) = extent(self.polygons.iter().flatten().flatten());
        // Every ring spans some latitude and longitude, so the extent is never flat.
        LLBBox::new(s, w, n, e).expect("checked shapes span a rectangle")
    }

    /// Whether the point lies in some shape, outside its holes; edges count as in, as for blocks.
    pub fn contains(&self, p: &LLPoint) -> bool {
        self.polygons.iter().any(|rings| {
            rings.iter().any(|ring| on_ring(ring, p))
                || rings
                    .iter()
                    .filter(|ring| {
                        let points: Vec<(f64, f64)> =
                            ring.iter().map(|q| (q.lat(), q.lng())).collect();
                        crate::osm_parser::point_in_ring(p.lat(), p.lng(), &points)
                    })
                    .count()
                    % 2
                    == 1
        })
    }
}

const ACROSS_MERIDIAN: &str = "it spans more than half the globe east to west, which is how an area across the 180° meridian looks; keep it on one side";

/// South, west, north and east edge of some points.
fn extent<'a>(points: impl IntoIterator<Item = &'a LLPoint>) -> (f64, f64, f64, f64) {
    points.into_iter().fold(
        (90.0_f64, 180.0_f64, -90.0_f64, -180.0_f64),
        |(s, w, n, e), p| {
            (
                s.min(p.lat()),
                w.min(p.lng()),
                n.max(p.lat()),
                e.max(p.lng()),
            )
        },
    )
}

fn check_ring(ring: Vec<(f64, f64)>) -> Result<Vec<LLPoint>, String> {
    let mut points: Vec<LLPoint> = Vec::with_capacity(ring.len());
    for (lat, lng) in ring {
        let p = LLPoint::new(lat, lng)?;
        if points.last() != Some(&p) {
            points.push(p);
        }
    }
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    if points.len() < 3 {
        return Err("an outline needs at least three different points".to_string());
    }
    let (s, w, n, e) = extent(&points);
    if e - w > 180.0 {
        return Err(ACROSS_MERIDIAN.to_string());
    }
    // About the first point, so a flat outline gives zero rather than rounding noise.
    let o = points[0];
    let twice_area: f64 = (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            let (ax, ay, bx, by) = (
                a.lng() - o.lng(),
                a.lat() - o.lat(),
                b.lng() - o.lng(),
                b.lat() - o.lat(),
            );
            ax * by - bx * ay
        })
        .sum();
    if n <= s || e <= w || twice_area.abs() <= 1e-9 * (n - s) * (e - w) {
        return Err("the outline encloses no area".to_string());
    }
    Ok(points)
}

fn on_ring(ring: &[LLPoint], p: &LLPoint) -> bool {
    let (x, y) = (p.lng(), p.lat());
    (0..ring.len()).any(|i| {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        let (ax, ay, bx, by) = (a.lng(), a.lat(), b.lng(), b.lat());
        let cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        cross.abs() <= 1e-12 * (1.0 + ax.abs().max(bx.abs()))
            && x >= ax.min(bx) - 1e-12
            && x <= ax.max(bx) + 1e-12
            && y >= ay.min(by) - 1e-12
            && y <= ay.max(by) + 1e-12
    })
}

fn geojson_ring(v: &serde_json::Value) -> Result<Vec<(f64, f64)>, String> {
    v.as_array()
        .ok_or("a ring is not a list of positions")?
        .iter()
        .map(|pos| {
            let pos = pos.as_array().ok_or("a position is not a list")?;
            match (
                pos.first().and_then(|v| v.as_f64()),
                pos.get(1).and_then(|v| v.as_f64()),
            ) {
                // GeoJSON puts longitude first.
                (Some(lng), Some(lat)) => Ok((lat, lng)),
                _ => Err("a position is not [longitude, latitude]".to_string()),
            }
        })
        .collect()
}

fn geojson_polygon(v: &serde_json::Value) -> Result<LLRings, String> {
    v.as_array()
        .ok_or("Polygon coordinates are not a list of rings")?
        .iter()
        .map(geojson_ring)
        .collect()
}

fn collect_geojson(
    v: &serde_json::Value,
    out: &mut Vec<LLRings>,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("GeoJSON nests too deep".to_string());
    }
    match v.get("type").and_then(|t| t.as_str()) {
        Some("FeatureCollection") => {
            for f in v
                .get("features")
                .and_then(|f| f.as_array())
                .ok_or("FeatureCollection has no features")?
            {
                collect_geojson(f, out, depth + 1)?;
            }
        }
        Some("Feature") => {
            if let Some(g) = v.get("geometry").filter(|g| !g.is_null()) {
                collect_geojson(g, out, depth + 1)?;
            }
        }
        Some("GeometryCollection") => {
            for g in v
                .get("geometries")
                .and_then(|g| g.as_array())
                .ok_or("GeometryCollection has no geometries")?
            {
                collect_geojson(g, out, depth + 1)?;
            }
        }
        Some("Polygon") => out.push(geojson_polygon(
            v.get("coordinates").ok_or("Polygon has no coordinates")?,
        )?),
        Some("MultiPolygon") => {
            for p in v
                .get("coordinates")
                .and_then(|c| c.as_array())
                .ok_or("MultiPolygon has no coordinates")?
            {
                out.push(geojson_polygon(p)?);
            }
        }
        // Points and lines enclose nothing.
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(lat: f64, lng: f64, d: f64) -> Vec<(f64, f64)> {
        vec![
            (lat, lng),
            (lat, lng + d),
            (lat + d, lng + d),
            (lat + d, lng),
        ]
    }

    #[test]
    fn bbox_covers_every_shape() {
        let s = LLShapes::new(vec![
            vec![square(52.5, 13.3, 0.01)],
            vec![square(52.6, 13.5, 0.02)],
        ])
        .unwrap();
        let b = s.bbox();
        assert_eq!((b.min().lat(), b.min().lng()), (52.5, 13.3));
        assert!((b.max().lat() - 52.62).abs() < 1e-12 && (b.max().lng() - 13.52).abs() < 1e-12);
    }

    #[test]
    fn rings_are_cleaned_and_checked() {
        let mut closed = square(1.0, 1.0, 1.0);
        closed.insert(1, closed[0]);
        closed.push(closed[0]);
        let s = LLShapes::new(vec![vec![closed]]).unwrap();
        assert_eq!(s.polygons()[0][0].len(), 4);

        assert!(LLShapes::new(vec![]).is_err());
        assert!(LLShapes::new(vec![vec![vec![(1.0, 1.0), (1.0, 2.0), (1.0, 1.0)]]]).is_err());
        assert!(LLShapes::new(vec![vec![vec![(1.0, 1.0), (1.0, 2.0), (1.0, 3.0)]]]).is_err());
        assert!(LLShapes::new(vec![vec![vec![(95.0, 1.0), (1.0, 2.0), (2.0, 3.0)]]]).is_err());
        let across = vec![(10.0, 179.0), (10.0, -179.0), (11.0, -179.0), (11.0, 179.0)];
        assert!(LLShapes::new(vec![vec![across]]).is_err());
        // Each fine on its own, but together they reach round the globe.
        assert!(LLShapes::new(vec![
            vec![square(10.0, 178.0, 1.0)],
            vec![square(10.0, -179.5, 1.0)]
        ])
        .is_err());
        // Flat along a parallel: wide enough that the area sums to rounding noise.
        let flat = vec![
            (54.012476, 19.19447),
            (54.012476, 48.380083),
            (54.012476, -106.79187),
        ];
        assert!(LLShapes::new(vec![vec![flat]]).is_err());
        let flat_ns = vec![(10.0, 5.0), (20.0, 5.0), (15.0, 5.0)];
        assert!(LLShapes::new(vec![vec![flat_ns]]).is_err());
        // A thin but real sliver is kept.
        assert!(LLShapes::new(vec![vec![vec![
            (52.5, 13.3),
            (52.5, 13.4),
            (52.50001, 13.35)
        ]]])
        .is_ok());
    }

    #[test]
    fn contains_follows_holes_and_merges() {
        let donut = vec![square(0.0, 0.0, 3.0), square(1.0, 1.0, 1.0)];
        let s = LLShapes::new(vec![donut, vec![square(1.2, 1.2, 0.5)]]).unwrap();
        assert!(s.contains(&LLPoint::new(0.5, 0.5).unwrap()));
        // In the hole, but inside the second shape.
        assert!(s.contains(&LLPoint::new(1.4, 1.4).unwrap()));
        assert!(!s.contains(&LLPoint::new(1.1, 1.9).unwrap()));
        assert!(s.contains(&LLPoint::new(0.0, 1.5).unwrap()));
        // A hole's edge is in, as the block grid has it.
        assert!(s.contains(&LLPoint::new(1.0, 1.5).unwrap()));
        assert!(!s.contains(&LLPoint::new(3.5, 1.0).unwrap()));
    }

    #[test]
    fn points_parse_with_spaces_or_semicolons() {
        // A space after the comma splits the point in two.
        assert!(LLShapes::parse_points("52.5,13.3 52.6, 13.4").is_err());
        let p = LLShapes::parse_points(" 52.5,13.3  52.6,13.3;52.6,13.4\n").unwrap();
        assert_eq!(p, vec![(52.5, 13.3), (52.6, 13.3), (52.6, 13.4)]);
        assert!(LLShapes::parse_points("52.5;13.3").is_err());
        assert!(LLShapes::parse_points("52.5,13.3,4").is_err());
        assert!(LLShapes::parse_points("52.5,x").is_err());
        assert!(LLShapes::parse_points("-33.9,-70.6").is_ok());
    }

    #[test]
    fn geojson_polygons_multipolygons_and_features() {
        let doc = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{},"geometry":{"type":"Polygon","coordinates":[
                [[13.3,52.5],[13.4,52.5],[13.4,52.6],[13.3,52.6],[13.3,52.5]],
                [[13.33,52.53],[13.36,52.53],[13.36,52.56],[13.33,52.53]]]}},
            {"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[
                [[[13.5,52.5],[13.6,52.5],[13.6,52.6],[13.5,52.5]]],
                [[[13.7,52.5],[13.8,52.5],[13.8,52.6],[13.7,52.5]]]]}},
            {"type":"Feature","geometry":{"type":"Point","coordinates":[13.0,52.0]}},
            {"type":"Feature","geometry":null}]}"#;
        let polys = LLShapes::from_geojson(doc).unwrap();
        assert_eq!(polys.len(), 3);
        assert_eq!(polys[0].len(), 2);
        assert_eq!(polys[0][0][0], (52.5, 13.3));
        let s = LLShapes::new(polys).unwrap();
        assert!(!s.contains(&LLPoint::new(52.535, 13.345).unwrap()));
        assert!(s.contains(&LLPoint::new(52.51, 13.31).unwrap()));

        assert!(LLShapes::from_geojson(r#"{"type":"Point","coordinates":[1,2]}"#).is_err());
        assert!(LLShapes::from_geojson("not json").is_err());
        assert!(LLShapes::from_geojson(r#"{"type":"Polygon","coordinates":[[[1,"a"]]]}"#).is_err());
    }
}
