//! Minimal Mapbox Vector Tile decoder.
//!
//! Only what Overture's building tiles need: layers, their key/value tables,
//! and polygon rings. Points and linestrings are decoded to rings too, but the
//! caller filters on `geom_type`.
//!
//! This parses bytes straight off the network, so every read is bounds-checked
//! and every count that comes out of the input is bounded by the input that is
//! actually left. A malformed tile returns an error or a short feature list; it
//! never panics and never allocates on a length the file merely claims.

/// Protobuf wire types we accept. Groups (3 and 4) are deprecated and rejected.
const WIRE_VARINT: u8 = 0;
const WIRE_64BIT: u8 = 1;
const WIRE_LEN: u8 = 2;
const WIRE_32BIT: u8 = 5;

/// MVT geometry type for a polygon (`vector_tile.proto`, `GeomType.POLYGON`).
pub const GEOM_POLYGON: u32 = 3;

/// Default tile extent when a layer omits it, per the MVT specification.
const DEFAULT_EXTENT: u32 = 4096;

/// A tile is one protobuf message; anything larger than this is not a tile we
/// asked for. Overture's densest z14 building tiles decompress to ~1.6 MB.
const MAX_TILE_BYTES: usize = 64 * 1024 * 1024;

pub type Result<T> = std::result::Result<T, String>;

/// One value from a layer's value table.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    F64(f64),
    I64(i64),
    U64(u64),
    Bool(bool),
}

impl Value {
    /// The value as a string, for attributes Overture encodes as text.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a number, accepting every numeric encoding a writer may
    /// have chosen. Overture's tiles store `num_floors` as an integer and
    /// `height` as a float, but nothing in the format guarantees that, and a
    /// re-encode with different types must not silently drop heights.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F64(v) => Some(*v),
            Value::I64(v) => Some(*v as f64),
            Value::U64(v) => Some(*v as f64),
            // A height written as the string "12.5" is still a height.
            Value::Str(s) => s.trim().parse().ok(),
            Value::Bool(_) => None,
        }
    }
}

/// A decoded polygon ring in tile-local integer coordinates.
#[derive(Debug, Clone)]
pub struct Ring {
    pub points: Vec<(i32, i32)>,
    /// Twice the ring's signed area, kept because callers need it both to tell
    /// an exterior ring from a hole and to pick the largest part of a
    /// multipolygon - and recomputing it per ring is pure waste.
    pub area2: i128,
}

impl Ring {
    /// Per the MVT specification exterior rings wind clockwise on screen, which
    /// in the tile's y-down coordinate system is a positive shoelace sum.
    pub fn exterior(&self) -> bool {
        self.area2 > 0
    }

    /// Twice the ring's unsigned area, for comparing parts of one polygon.
    pub fn area2_abs(&self) -> i128 {
        self.area2.unsigned_abs() as i128
    }
}

/// One feature. Attributes stay as indices into the layer's tables so a tile
/// with thousands of features allocates no string per attribute.
#[derive(Debug, Clone)]
pub struct Feature {
    pub geom_type: u32,
    /// Flat `(key_index, value_index)` pairs, as the format stores them.
    pub tags: Vec<u32>,
    pub rings: Vec<Ring>,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub name: String,
    pub extent: u32,
    pub keys: Vec<String>,
    pub values: Vec<Value>,
    pub features: Vec<Feature>,
}

impl Layer {
    /// Look up one attribute of a feature by key name.
    pub fn attr(&self, feature: &Feature, key: &str) -> Option<&Value> {
        // Tags are (key, value) pairs; an odd trailing entry is malformed and
        // simply has no pair to read.
        for &[key_index, value_index] in feature.tags.as_chunks::<2>().0 {
            let (k, v) = (key_index as usize, value_index as usize);
            if self.keys.get(k).is_some_and(|name| name == key) {
                return self.values.get(v);
            }
        }
        None
    }

    /// The attribute as a string, for the common case.
    pub fn attr_str(&self, feature: &Feature, key: &str) -> Option<&str> {
        self.attr(feature, key).and_then(Value::as_str)
    }

    /// The attribute as a number, for the common case.
    pub fn attr_f64(&self, feature: &Feature, key: &str) -> Option<f64> {
        self.attr(feature, key).and_then(Value::as_f64)
    }
}

// ─── Protobuf wire reader ────────────────────────────────────────────────

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn varint(&mut self) -> Result<u64> {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = *self
                .buf
                .get(self.pos)
                .ok_or("truncated varint at end of buffer")?;
            self.pos += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            // A protobuf varint is at most 10 bytes; past that the input is
            // malformed and continuing would shift out of range.
            if shift >= 64 {
                return Err("varint longer than 64 bits".into());
            }
        }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        if len > self.remaining() {
            return Err(format!(
                "length-delimited field claims {len} bytes, {} remain",
                self.remaining()
            ));
        }
        let out = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok(out)
    }

    /// Next `(field_number, wire_type)`, or `None` at end of buffer.
    fn key(&mut self) -> Result<Option<(u32, u8)>> {
        if self.done() {
            return Ok(None);
        }
        let key = self.varint()?;
        let field = u32::try_from(key >> 3).map_err(|_| "field number out of range")?;
        Ok(Some((field, (key & 7) as u8)))
    }

    /// Consume the value for a field we do not care about.
    fn skip(&mut self, wire: u8) -> Result<()> {
        match wire {
            WIRE_VARINT => {
                self.varint()?;
            }
            WIRE_64BIT => {
                self.bytes(8)?;
            }
            WIRE_LEN => {
                let len = usize::try_from(self.varint()?).map_err(|_| "field length overflow")?;
                self.bytes(len)?;
            }
            WIRE_32BIT => {
                self.bytes(4)?;
            }
            other => return Err(format!("unsupported protobuf wire type {other}")),
        }
        Ok(())
    }

    /// A length-delimited submessage.
    fn message(&mut self) -> Result<Reader<'a>> {
        let len = usize::try_from(self.varint()?).map_err(|_| "message length overflow")?;
        Ok(Reader::new(self.bytes(len)?))
    }

    fn string(&mut self) -> Result<String> {
        let len = usize::try_from(self.varint()?).map_err(|_| "string length overflow")?;
        let raw = self.bytes(len)?;
        // Overture writes UTF-8; a broken byte in a name must cost that name,
        // not the tile.
        Ok(String::from_utf8_lossy(raw).into_owned())
    }

    /// A packed repeated `uint32`. Also accepts the unpacked encoding, which
    /// the specification still permits.
    fn packed_u32(&mut self, wire: u8, out: &mut Vec<u32>) -> Result<()> {
        match wire {
            WIRE_VARINT => {
                out.push(u32::try_from(self.varint()?).map_err(|_| "u32 out of range")?);
            }
            WIRE_LEN => {
                let mut inner = self.message()?;
                // Deliberately no `reserve` here. The byte length bounds the
                // element count, but each element is four bytes, so reserving
                // from it would allocate four times a field size that the tile
                // itself chooses. Amortised growth costs less than trusting it.
                while !inner.done() {
                    out.push(u32::try_from(inner.varint()?).map_err(|_| "u32 out of range")?);
                }
            }
            other => return Err(format!("packed u32 field has wire type {other}")),
        }
        Ok(())
    }
}

fn zigzag(value: u32) -> i32 {
    ((value >> 1) as i32) ^ -((value & 1) as i32)
}

// ─── Message decoding ────────────────────────────────────────────────────

/// Decode a tile into its layers.
pub fn decode_tile(buf: &[u8]) -> Result<Vec<Layer>> {
    if buf.len() > MAX_TILE_BYTES {
        return Err(format!("tile is {} bytes, refusing to decode", buf.len()));
    }
    let mut reader = Reader::new(buf);
    let mut layers = Vec::new();
    while let Some((field, wire)) = reader.key()? {
        match (field, wire) {
            // Tile.layers
            (3, WIRE_LEN) => layers.push(decode_layer(&mut reader.message()?)?),
            _ => reader.skip(wire)?,
        }
    }
    Ok(layers)
}

fn decode_layer(reader: &mut Reader<'_>) -> Result<Layer> {
    let mut name = String::new();
    let mut extent = DEFAULT_EXTENT;
    let mut keys: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    // Features are decoded after the whole layer, because a writer is free to
    // emit them before the extent that their geometry is expressed in.
    let mut feature_bodies: Vec<&[u8]> = Vec::new();

    while let Some((field, wire)) = reader.key()? {
        match (field, wire) {
            (1, WIRE_LEN) => name = reader.string()?,
            (2, WIRE_LEN) => {
                let len = usize::try_from(reader.varint()?).map_err(|_| "feature length")?;
                feature_bodies.push(reader.bytes(len)?);
            }
            (3, WIRE_LEN) => keys.push(reader.string()?),
            (4, WIRE_LEN) => values.push(decode_value(&mut reader.message()?)?),
            (5, WIRE_VARINT) => {
                let raw = reader.varint()?;
                // Extent divides every coordinate; zero would make the tile
                // transform undefined, so keep the specified default instead.
                extent = u32::try_from(raw).unwrap_or(DEFAULT_EXTENT).max(1);
            }
            _ => reader.skip(wire)?,
        }
    }

    let mut features = Vec::with_capacity(feature_bodies.len());
    for body in feature_bodies {
        // One unreadable feature costs that feature, not the layer: a tile with
        // thousands of buildings should not be lost to one bad geometry.
        if let Ok(feature) = decode_feature(&mut Reader::new(body)) {
            features.push(feature);
        }
    }

    Ok(Layer {
        name,
        extent,
        keys,
        values,
        features,
    })
}

fn decode_value(reader: &mut Reader<'_>) -> Result<Value> {
    let mut value = Value::Bool(false);
    while let Some((field, wire)) = reader.key()? {
        match (field, wire) {
            (1, WIRE_LEN) => value = Value::Str(reader.string()?),
            (2, WIRE_32BIT) => {
                let b = reader.bytes(4)?;
                value = Value::F64(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64);
            }
            (3, WIRE_64BIT) => {
                let b = reader.bytes(8)?;
                let mut arr = [0u8; 8];
                arr.copy_from_slice(b);
                value = Value::F64(f64::from_le_bytes(arr));
            }
            (4, WIRE_VARINT) => value = Value::I64(reader.varint()? as i64),
            (5, WIRE_VARINT) => value = Value::U64(reader.varint()?),
            (6, WIRE_VARINT) => {
                let raw = reader.varint()?;
                value = Value::I64(((raw >> 1) as i64) ^ -((raw & 1) as i64));
            }
            (7, WIRE_VARINT) => value = Value::Bool(reader.varint()? != 0),
            _ => reader.skip(wire)?,
        }
    }
    Ok(value)
}

fn decode_feature(reader: &mut Reader<'_>) -> Result<Feature> {
    let mut geom_type = 0u32;
    let mut tags: Vec<u32> = Vec::new();
    let mut geometry: Vec<u32> = Vec::new();

    while let Some((field, wire)) = reader.key()? {
        match field {
            // Feature.id - not used; Overture's own `id` attribute is the GERS id.
            1 => reader.skip(wire)?,
            2 => reader.packed_u32(wire, &mut tags)?,
            3 if wire == WIRE_VARINT => {
                geom_type = u32::try_from(reader.varint()?).unwrap_or(0);
            }
            4 => reader.packed_u32(wire, &mut geometry)?,
            _ => reader.skip(wire)?,
        }
    }

    Ok(Feature {
        geom_type,
        tags,
        rings: decode_geometry(&geometry),
    })
}

/// Decode MVT command/parameter integers into rings.
///
/// Commands are `(id & 0x7) | (count << 3)`; `MoveTo` (1) and `LineTo` (2) each
/// take two zigzag parameters per repetition, `ClosePath` (7) takes none.
/// Coordinates are cursor deltas, so a truncated stream simply ends the ring
/// rather than corrupting the ones already read.
fn decode_geometry(geometry: &[u32]) -> Vec<Ring> {
    let mut rings: Vec<Ring> = Vec::new();
    let mut current: Vec<(i32, i32)> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0usize;

    let flush = |points: &mut Vec<(i32, i32)>, rings: &mut Vec<Ring>| {
        // Two points cannot bound an area; a degenerate ring is dropped rather
        // than passed on as a zero-area polygon.
        if points.len() >= 3 {
            let ring = std::mem::take(points);
            let area2 = shoelace2(&ring);
            rings.push(Ring {
                points: ring,
                area2,
            });
        } else {
            points.clear();
        }
    };

    while i < geometry.len() {
        let command = geometry[i];
        i += 1;
        let id = command & 0x7;
        let count = (command >> 3) as usize;

        match id {
            // MoveTo: starts a new ring at an absolute (delta-accumulated) point.
            1 => {
                for _ in 0..count {
                    let Some(&dx) = geometry.get(i) else { break };
                    let Some(&dy) = geometry.get(i + 1) else {
                        break;
                    };
                    i += 2;
                    x = x.wrapping_add(zigzag(dx));
                    y = y.wrapping_add(zigzag(dy));
                    flush(&mut current, &mut rings);
                    current.push((x, y));
                }
            }
            // LineTo: extends the current ring.
            2 => {
                current.reserve(count.min(geometry.len().saturating_sub(i) / 2));
                for _ in 0..count {
                    let Some(&dx) = geometry.get(i) else { break };
                    let Some(&dy) = geometry.get(i + 1) else {
                        break;
                    };
                    i += 2;
                    x = x.wrapping_add(zigzag(dx));
                    y = y.wrapping_add(zigzag(dy));
                    current.push((x, y));
                }
            }
            // ClosePath: the ring is implicitly closed, so nothing is appended.
            7 => flush(&mut current, &mut rings),
            // An unknown command id means the rest of the stream cannot be
            // located; keep what has been read.
            _ => break,
        }
    }
    flush(&mut current, &mut rings);
    rings
}

/// Twice the signed area of a ring. Positive means clockwise on screen in the
/// tile's y-down coordinate system, which the MVT specification defines as an
/// exterior ring.
///
/// `i128`, not `i64`. Coordinates accumulate from cursor deltas and can reach
/// any `i32`, so one term is up to 2^62 and just three of them overflow an
/// `i64` accumulator - which this crate builds with `overflow-checks = true`,
/// so it would panic in release on a malformed tile rather than merely give a
/// wrong sign. This function is on the no-panic path for network data.
fn shoelace2(points: &[(i32, i32)]) -> i128 {
    let mut sum: i128 = 0;
    for idx in 0..points.len() {
        let (x1, y1) = points[idx];
        let (x2, y2) = points[(idx + 1) % points.len()];
        sum += i128::from(x1) * i128::from(y2) - i128::from(x2) * i128::from(y1);
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a protobuf varint.
    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn key(field: u32, wire: u8) -> Vec<u8> {
        varint((u64::from(field) << 3) | u64::from(wire))
    }

    fn len_field(field: u32, body: &[u8]) -> Vec<u8> {
        let mut out = key(field, WIRE_LEN);
        out.extend(varint(body.len() as u64));
        out.extend_from_slice(body);
        out
    }

    fn zz(v: i32) -> u32 {
        ((v << 1) ^ (v >> 31)) as u32
    }

    fn packed(values: &[u32]) -> Vec<u8> {
        let mut body = Vec::new();
        for v in values {
            body.extend(varint(u64::from(*v)));
        }
        body
    }

    /// One square building, clockwise on screen, with a string and a number.
    fn sample_tile() -> Vec<u8> {
        let geometry = packed(&[
            (1 << 3) | 1, // MoveTo, 1
            zz(10),
            zz(10),
            (3 << 3) | 2, // LineTo, 3
            zz(20),
            zz(0),
            zz(0),
            zz(20),
            zz(-20),
            zz(0),
            7 | (1 << 3), // ClosePath
        ]);

        let mut feature = Vec::new();
        feature.extend(len_field(2, &packed(&[0, 0, 1, 1]))); // tags
        feature.extend(key(3, WIRE_VARINT));
        feature.extend(varint(u64::from(GEOM_POLYGON)));
        feature.extend(len_field(4, &geometry));

        let mut string_value = Vec::new();
        string_value.extend(len_field(1, b"OpenStreetMap"));
        let mut double_value = Vec::new();
        double_value.extend(key(3, WIRE_64BIT));
        double_value.extend_from_slice(&12.5f64.to_le_bytes());

        let mut layer = Vec::new();
        layer.extend(len_field(1, b"building"));
        layer.extend(len_field(2, &feature));
        layer.extend(len_field(3, b"@geometry_source"));
        layer.extend(len_field(3, b"height"));
        layer.extend(len_field(4, &string_value));
        layer.extend(len_field(4, &double_value));
        layer.extend(key(5, WIRE_VARINT));
        layer.extend(varint(4096));

        len_field(3, &layer)
    }

    #[test]
    fn decodes_a_layer_with_attributes_and_one_exterior_ring() {
        let layers = decode_tile(&sample_tile()).unwrap();
        assert_eq!(layers.len(), 1);
        let layer = &layers[0];
        assert_eq!(layer.name, "building");
        assert_eq!(layer.extent, 4096);
        assert_eq!(layer.features.len(), 1);

        let feature = &layer.features[0];
        assert_eq!(feature.geom_type, GEOM_POLYGON);
        assert_eq!(
            layer.attr_str(feature, "@geometry_source"),
            Some("OpenStreetMap")
        );
        assert_eq!(layer.attr_f64(feature, "height"), Some(12.5));
        assert_eq!(layer.attr(feature, "absent"), None);

        assert_eq!(feature.rings.len(), 1);
        let ring = &feature.rings[0];
        assert!(ring.exterior(), "clockwise-on-screen ring must be exterior");
        assert_eq!(
            ring.points,
            vec![(10, 10), (30, 10), (30, 30), (10, 30)],
            "cursor deltas must accumulate"
        );
    }

    #[test]
    fn a_counter_clockwise_ring_is_a_hole() {
        // Same square, wound the other way.
        let ring = vec![(10, 10), (10, 30), (30, 30), (30, 10)];
        assert!(shoelace2(&ring) < 0);
        let rings = decode_geometry(&[
            (1 << 3) | 1,
            zz(10),
            zz(10),
            (3 << 3) | 2,
            zz(0),
            zz(20),
            zz(20),
            zz(0),
            zz(0),
            zz(-20),
            7 | (1 << 3),
        ]);
        assert_eq!(rings.len(), 1);
        assert!(!rings[0].exterior());
    }

    #[test]
    fn a_multipolygon_yields_one_exterior_ring_per_part() {
        // Two separate squares in one feature.
        let mut cmds = vec![(1 << 3) | 1, zz(0), zz(0), (3 << 3) | 2];
        cmds.extend([zz(10), zz(0), zz(0), zz(10), zz(-10), zz(0)]);
        cmds.push(7 | (1 << 3));
        cmds.extend([(1 << 3) | 1, zz(50), zz(0), (3 << 3) | 2]);
        cmds.extend([zz(10), zz(0), zz(0), zz(10), zz(-10), zz(0)]);
        cmds.push(7 | (1 << 3));

        let rings = decode_geometry(&cmds);
        assert_eq!(rings.len(), 2);
        assert!(rings.iter().all(|r| r.exterior()));
    }

    #[test]
    fn truncated_and_malformed_input_is_rejected_without_panicking() {
        let tile = sample_tile();
        // Every prefix of a valid tile must either decode or error, never panic.
        for cut in 0..tile.len() {
            let _ = decode_tile(&tile[..cut]);
        }
        // A varint that never terminates.
        assert!(decode_tile(&[0x1a, 0x02, 0xff, 0xff]).is_err());
        // A length-delimited field claiming more bytes than exist.
        assert!(decode_tile(&[0x1a, 0x7f, 0x00]).is_err());
        // Deprecated group wire types are refused rather than mis-parsed.
        assert!(decode_tile(&[0x1b]).is_err());
    }

    #[test]
    fn a_geometry_that_runs_short_keeps_the_points_it_had() {
        // LineTo claims three points but only supplies one.
        let rings = decode_geometry(&[
            (1 << 3) | 1,
            zz(0),
            zz(0),
            (3 << 3) | 2,
            zz(10),
            zz(0),
            zz(0),
            zz(10),
        ]);
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].points, vec![(0, 0), (10, 0), (10, 10)]);
    }

    #[test]
    fn an_extreme_ring_computes_its_area_without_overflowing() {
        // Coordinates accumulate from cursor deltas and can reach any i32. With
        // an i64 accumulator three of these terms overflow, and this crate
        // builds with overflow-checks on, so that would panic in release.
        let extreme = vec![
            (i32::MIN, i32::MIN),
            (i32::MAX, i32::MIN),
            (i32::MAX, i32::MAX),
            (i32::MIN, i32::MAX),
        ];
        let area2 = shoelace2(&extreme);
        assert!(area2.unsigned_abs() > u64::MAX as u128, "area needs i128");

        // And the same through the public decoder, which is the contract that
        // matters: malformed input must not panic.
        let rings = decode_geometry(&[
            (1 << 3) | 1,
            zz(i32::MIN),
            zz(i32::MIN),
            (3 << 3) | 2,
            zz(i32::MAX),
            zz(0),
            zz(0),
            zz(i32::MAX),
            zz(i32::MIN),
            zz(0),
            7 | (1 << 3),
        ]);
        assert_eq!(rings.len(), 1);
    }

    #[test]
    fn the_largest_part_of_a_multipolygon_is_the_one_with_the_most_area() {
        // A big plain square and a small many-sided one. Picking by vertex
        // count would choose the outbuilding over the building.
        let mut cmds = vec![(1 << 3) | 1, zz(0), zz(0), (3 << 3) | 2];
        cmds.extend([zz(1000), zz(0), zz(0), zz(1000), zz(-1000), zz(0)]);
        cmds.push(7 | (1 << 3));
        cmds.extend([(1 << 3) | 1, zz(2000), zz(0), (7 << 3) | 2]);
        for step in [
            (10, 0),
            (10, 0),
            (0, 10),
            (0, 10),
            (-10, 0),
            (-10, 0),
            (0, -10),
        ] {
            cmds.extend([zz(step.0), zz(step.1)]);
        }
        cmds.push(7 | (1 << 3));

        let rings = decode_geometry(&cmds);
        assert_eq!(rings.len(), 2);
        assert!(rings[1].points.len() > rings[0].points.len(), "setup");
        let largest = rings.iter().max_by_key(|r| r.area2_abs()).unwrap();
        assert_eq!(largest.points.len(), rings[0].points.len());
    }

    #[test]
    fn degenerate_rings_are_dropped() {
        // Two points cannot bound an area.
        let rings = decode_geometry(&[(1 << 3) | 1, zz(0), zz(0), (1 << 3) | 2, zz(5), zz(5)]);
        assert!(rings.is_empty());
    }

    #[test]
    fn numbers_are_read_from_every_encoding_a_writer_may_use() {
        assert_eq!(Value::F64(3.5).as_f64(), Some(3.5));
        assert_eq!(Value::I64(7).as_f64(), Some(7.0));
        assert_eq!(Value::U64(9).as_f64(), Some(9.0));
        assert_eq!(Value::Str("12.5".into()).as_f64(), Some(12.5));
        assert_eq!(Value::Str("tall".into()).as_f64(), None);
        assert_eq!(Value::Bool(true).as_f64(), None);
    }
}
