//! The annotation ("ink") model and its binary encoding.
//!
//! A page layer is a list of objects drawn on top of one PDF page. The PDF
//! itself is never modified. Coordinates are relative to the page: `x` runs
//! from 0 (left edge) to [`COORD_MAX`] (right edge), `y` from 0 (top) to
//! [`COORD_MAX`] (bottom). This makes layers independent of zoom, screen
//! size and device. Lengths (stroke width, stamp size) are in units of
//! 1/[`COORD_MAX`] of the page width.
//!
//! The wire and storage format is one version byte followed by the
//! postcard (serde) encoding of [`PageLayer`]. Stroke points are delta
//! encoded, so most coordinates take a single byte. The server additionally
//! compresses stored layers with zstd.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Version byte that prefixes every encoded layer.
pub const FORMAT_VERSION: u8 = 1;

/// Page coordinates run from 0 to this value on both axes.
pub const COORD_MAX: u16 = u16::MAX;

/// Upper bounds a decoded layer must respect. They keep a single save from
/// exhausting server memory and are far above what a musician draws.
pub const MAX_OBJECTS_PER_PAGE: usize = 20_000;
pub const MAX_POINTS_PER_STROKE: usize = 20_000;
pub const MAX_TEXT_CHARS: usize = 1_000;
pub const MAX_ENCODED_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct PageLayer {
    pub objects: Vec<InkObject>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum InkObject {
    Stroke(Stroke),
    Stamp(Stamp),
    Text(TextNote),
}

impl InkObject {
    /// Random 64-bit identifier, unique within a set. Merging and erasing
    /// work per object id.
    pub fn id(&self) -> u64 {
        match self {
            InkObject::Stroke(s) => s.id,
            InkObject::Stamp(s) => s.id,
            InkObject::Text(t) => t.id,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    /// Drawn semi-transparent and behind pen strokes.
    Highlighter,
}

/// A freehand stroke. Use [`Stroke::from_points`] and [`Stroke::points`]
/// rather than the delta fields directly.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Stroke {
    pub id: u64,
    pub tool: Tool,
    /// `0xRRGGBBAA`.
    pub color: u32,
    pub width: u16,
    pub start: [u16; 2],
    /// Difference of each following point to its predecessor.
    pub deltas: Vec<[i32; 2]>,
    /// Pen pressure per point (0–255), one more entry than `deltas`.
    pub pressure: Vec<u8>,
}

impl Stroke {
    /// Builds a stroke from absolute points `(x, y, pressure)`.
    ///
    /// # Panics
    /// If `points` is empty.
    pub fn from_points(id: u64, tool: Tool, color: u32, width: u16, points: &[(u16, u16, u8)]) -> Self {
        let first = points.first().expect("a stroke needs at least one point");
        let deltas =
            points.windows(2).map(|w| [w[1].0 as i32 - w[0].0 as i32, w[1].1 as i32 - w[0].1 as i32]).collect();
        Self {
            id,
            tool,
            color,
            width,
            start: [first.0, first.1],
            deltas,
            pressure: points.iter().map(|p| p.2).collect(),
        }
    }

    /// Absolute points `(x, y)`.
    pub fn points(&self) -> Vec<(u16, u16)> {
        let (mut x, mut y) = (self.start[0] as i32, self.start[1] as i32);
        let mut out = Vec::with_capacity(self.deltas.len() + 1);
        out.push((x as u16, y as u16));
        for d in &self.deltas {
            x = (x + d[0]).clamp(0, COORD_MAX as i32);
            y = (y + d[1]).clamp(0, COORD_MAX as i32);
            out.push((x as u16, y as u16));
        }
        out
    }
}

/// Common musical marks placed with one tap.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampKind {
    BreathMark,
    Fermata,
    /// "Watch the conductor."
    Glasses,
    Sharp,
    Flat,
    Natural,
    Accent,
    Caesura,
}

impl StampKind {
    pub const ALL: [StampKind; 8] = [
        StampKind::BreathMark,
        StampKind::Fermata,
        StampKind::Glasses,
        StampKind::Sharp,
        StampKind::Flat,
        StampKind::Natural,
        StampKind::Accent,
        StampKind::Caesura,
    ];

    /// The glyph clients draw for the stamp.
    pub fn glyph(self) -> &'static str {
        match self {
            StampKind::BreathMark => "\u{2019}",
            StampKind::Fermata => "\u{1D110}",
            StampKind::Glasses => "\u{1F453}",
            StampKind::Sharp => "\u{266F}",
            StampKind::Flat => "\u{266D}",
            StampKind::Natural => "\u{266E}",
            StampKind::Accent => ">",
            StampKind::Caesura => "//",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Stamp {
    pub id: u64,
    pub kind: StampKind,
    /// Centre of the stamp.
    pub at: [u16; 2],
    pub size: u16,
    pub color: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TextNote {
    pub id: u64,
    /// Left end of the text baseline.
    pub at: [u16; 2],
    pub size: u16,
    pub color: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InkError {
    Empty,
    UnsupportedVersion(u8),
    TooLarge,
    Malformed(String),
    LimitExceeded(&'static str),
}

impl std::fmt::Display for InkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InkError::Empty => write!(f, "empty annotation layer"),
            InkError::UnsupportedVersion(v) => write!(f, "unsupported annotation format version {v}"),
            InkError::TooLarge => write!(f, "annotation layer larger than {MAX_ENCODED_BYTES} bytes"),
            InkError::Malformed(e) => write!(f, "malformed annotation layer: {e}"),
            InkError::LimitExceeded(what) => write!(f, "annotation layer exceeds the limit for {what}"),
        }
    }
}

impl std::error::Error for InkError {}

/// Encodes a layer as version byte + postcard.
pub fn encode(layer: &PageLayer) -> Vec<u8> {
    let mut out = vec![FORMAT_VERSION];
    out.extend(postcard::to_allocvec(layer).expect("PageLayer always serialises"));
    out
}

/// Decodes and validates a layer produced by [`encode`].
pub fn decode(bytes: &[u8]) -> Result<PageLayer, InkError> {
    let (&version, body) = bytes.split_first().ok_or(InkError::Empty)?;
    if version != FORMAT_VERSION {
        return Err(InkError::UnsupportedVersion(version));
    }
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(InkError::TooLarge);
    }
    let layer: PageLayer = postcard::from_bytes(body).map_err(|e| InkError::Malformed(e.to_string()))?;
    validate(&layer)?;
    Ok(layer)
}

fn validate(layer: &PageLayer) -> Result<(), InkError> {
    if layer.objects.len() > MAX_OBJECTS_PER_PAGE {
        return Err(InkError::LimitExceeded("objects per page"));
    }
    let mut ids = HashSet::with_capacity(layer.objects.len());
    for object in &layer.objects {
        if !ids.insert(object.id()) {
            return Err(InkError::Malformed(format!("duplicate object id {}", object.id())));
        }
        match object {
            InkObject::Stroke(s) => {
                if s.deltas.len() + 1 > MAX_POINTS_PER_STROKE {
                    return Err(InkError::LimitExceeded("points per stroke"));
                }
                if s.pressure.len() != s.deltas.len() + 1 {
                    return Err(InkError::Malformed("pressure and point counts differ".into()));
                }
            }
            InkObject::Text(t) if t.text.chars().count() > MAX_TEXT_CHARS => {
                return Err(InkError::LimitExceeded("text length"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Three-way merge of a page edited on two devices of the same user.
///
/// - `base_ids`: object ids of the version both sides started from.
/// - `local`: this device's layer.
/// - `server`: the newer layer saved by another device.
///
/// Objects are never edited in place, only added or removed, so a merge by
/// id is exact: an object survives unless one side deleted it, and objects
/// added on either side are kept (local order first, then remote additions).
pub fn merge(base_ids: &HashSet<u64>, local: &PageLayer, server: &PageLayer) -> PageLayer {
    let server_ids: HashSet<u64> = server.objects.iter().map(InkObject::id).collect();
    let local_ids: HashSet<u64> = local.objects.iter().map(InkObject::id).collect();
    let mut objects: Vec<InkObject> = local
        .objects
        .iter()
        // In the base but gone from the server: deleted remotely.
        .filter(|o| !(base_ids.contains(&o.id()) && !server_ids.contains(&o.id())))
        .cloned()
        .collect();
    objects.extend(
        server
            .objects
            .iter()
            // Absent locally but in the base: deleted locally.
            .filter(|o| !local_ids.contains(&o.id()) && !base_ids.contains(&o.id()))
            .cloned(),
    );
    PageLayer { objects }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(id: u64, n: u16) -> InkObject {
        let points: Vec<(u16, u16, u8)> = (0..n).map(|i| (1000 + i * 37, 2000 + (i * 13) % 200, 128)).collect();
        InkObject::Stroke(Stroke::from_points(id, Tool::Pen, 0xd0021bff, 160, &points))
    }

    fn ids(layer: &PageLayer) -> Vec<u64> {
        layer.objects.iter().map(InkObject::id).collect()
    }

    #[test]
    fn round_trip_keeps_every_object() {
        let layer = PageLayer {
            objects: vec![
                stroke(1, 60),
                InkObject::Stamp(Stamp {
                    id: 2,
                    kind: StampKind::Glasses,
                    at: [30_000, 40_000],
                    size: 2_000,
                    color: 0x000000ff,
                }),
                InkObject::Text(TextNote {
                    id: 3,
                    at: [100, 200],
                    size: 1_500,
                    color: 0x0000ffff,
                    text: "Cut bars 32–40".into(),
                }),
            ],
        };
        let bytes = encode(&layer);
        assert_eq!(bytes[0], FORMAT_VERSION);
        assert_eq!(decode(&bytes).unwrap(), layer);
    }

    #[test]
    fn points_survive_delta_encoding_including_extremes() {
        let pts = [(0, 0, 0), (COORD_MAX, COORD_MAX, 255), (0, COORD_MAX, 7), (12_345, 54_321, 9)];
        let s = Stroke::from_points(9, Tool::Highlighter, 0, 1, &pts);
        assert_eq!(s.points(), pts.iter().map(|p| (p.0, p.1)).collect::<Vec<_>>());
    }

    #[test]
    fn typical_page_stays_small() {
        let layer = PageLayer { objects: (0..30).map(|i| stroke(i, 60)).collect() };
        // 30 strokes x 60 points; uncompressed, before the server's zstd.
        assert!(encode(&layer).len() < 12_000, "{} bytes", encode(&layer).len());
    }

    #[test]
    fn decode_rejects_bad_input() {
        assert_eq!(decode(&[]), Err(InkError::Empty));
        assert_eq!(decode(&[9, 0]), Err(InkError::UnsupportedVersion(9)));
        assert!(matches!(decode(&[FORMAT_VERSION, 0xff, 0xff]), Err(InkError::Malformed(_))));

        let dup = PageLayer { objects: vec![stroke(1, 2), stroke(1, 3)] };
        assert!(matches!(decode(&encode(&dup)), Err(InkError::Malformed(_))));

        let mut bad = Stroke::from_points(1, Tool::Pen, 0, 1, &[(0, 0, 0), (1, 1, 1)]);
        bad.pressure.pop();
        let layer = PageLayer { objects: vec![InkObject::Stroke(bad)] };
        assert!(matches!(decode(&encode(&layer)), Err(InkError::Malformed(_))));
    }

    #[test]
    fn merge_keeps_both_sides_additions_and_deletions() {
        let base = PageLayer { objects: vec![stroke(1, 3), stroke(2, 3), stroke(3, 3)] };
        let base_ids: HashSet<u64> = ids(&base).into_iter().collect();
        // This device deleted 2 and added 10; the other deleted 3 and added 20.
        let local = PageLayer { objects: vec![stroke(1, 3), stroke(3, 3), stroke(10, 3)] };
        let server = PageLayer { objects: vec![stroke(1, 3), stroke(2, 3), stroke(20, 3)] };
        assert_eq!(ids(&merge(&base_ids, &local, &server)), vec![1, 10, 20]);
    }

    #[test]
    fn merge_with_empty_base_is_a_union() {
        let local = PageLayer { objects: vec![stroke(1, 3)] };
        let server = PageLayer { objects: vec![stroke(1, 3), stroke(2, 3)] };
        assert_eq!(ids(&merge(&HashSet::new(), &local, &server)), vec![1, 2]);
    }
}
