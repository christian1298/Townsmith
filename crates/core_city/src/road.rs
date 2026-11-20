//! Axis-aligned road segments on the 1 m terrain grid.
//!
//! This is the first road slice: straight, horizontal or vertical segments
//! with a width and a shared store. Curves, lanes, intersections, sidewalks,
//! utilities, and traffic are later work. Terrain heights arrive through a
//! sampler so the core stays independent of `core_world`.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::placement::{MAX_WORLD_COORD_M, WATER_LEVEL_M};

/// Current road save format.
pub const ROAD_FORMAT_VERSION: u16 = 1;

/// Longest single segment in metres.
pub const MAX_ROAD_LENGTH_M: i32 = 512;

/// Narrowest allowed road.
pub const MIN_ROAD_WIDTH_M: u8 = 2;

/// Widest allowed road.
pub const MAX_ROAD_WIDTH_M: u8 = 24;

/// Maximum rise/fall across a segment's sampled width.
pub const MAX_ROAD_SLOPE_M: u16 = 8;

/// Hard cap for one save.
pub const MAX_ROADS: usize = 200_000;

/// Largest id representable by the GDScript bridge.
pub const MAX_ROAD_ID: u64 = i64::MAX as u64;

/// One straight road segment on the 1 m grid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadSegment {
    /// Stable id within the world.
    pub id: u64,
    /// Start centreline cell `[x, z]`.
    pub start: [i32; 2],
    /// End centreline cell `[x, z]`.
    pub end: [i32; 2],
    /// Road width in metres.
    pub width_m: u8,
}

impl RoadSegment {
    /// Segment length in metres. One axis is always zero.
    #[must_use]
    pub fn length_m(&self) -> i64 {
        i64::from(self.end[0] - self.start[0]).abs() + i64::from(self.end[1] - self.start[1]).abs()
    }

    /// Centre of the segment in the horizontal plane.
    #[must_use]
    pub fn center(&self) -> [f32; 2] {
        [
            (self.start[0] + self.end[0]) as f32 * 0.5,
            (self.start[1] + self.end[1]) as f32 * 0.5,
        ]
    }

    /// Whether the segment runs along X (otherwise Z).
    #[must_use]
    pub const fn runs_along_x(&self) -> bool {
        self.start[0] != self.end[0]
    }
}

/// A request to add one segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoadRequest {
    /// Start centreline cell `[x, z]`.
    pub start: [i32; 2],
    /// End centreline cell `[x, z]`.
    pub end: [i32; 2],
    /// Road width in metres.
    pub width_m: u8,
}

/// Road validation and persistence errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoadError {
    /// Start and end are the same cell.
    ZeroLength,
    /// The segment is not horizontal or vertical.
    NotAxisAligned,
    /// The segment is too long for one piece.
    TooLong {
        /// Requested length.
        length: i64,
    },
    /// Width is outside the allowed range.
    InvalidWidth {
        /// Requested width.
        width: u8,
    },
    /// A coordinate is outside the world range.
    CoordinateOutOfRange,
    /// A sampled road cell is at or below water.
    Water {
        /// Cell X.
        x: i32,
        /// Cell Z.
        z: i32,
    },
    /// Terrain rise/fall across the road exceeds the limit.
    SlopeTooSteep {
        /// Sampled rise in metres.
        rise: u16,
    },
    /// The store is full.
    TooMany,
    /// JSON could not be parsed.
    InvalidJson(String),
    /// The save format version is not supported.
    UnsupportedVersion {
        /// Found version.
        version: u16,
    },
    /// Two roads share an id.
    DuplicateId {
        /// Duplicate id.
        id: u64,
    },
    /// A loaded record is inconsistent.
    InvalidRecord {
        /// Record id.
        id: u64,
        /// Short reason.
        reason: &'static str,
    },
    /// `next_id` is zero or not greater than every stored id.
    InvalidNextId {
        /// Found next id.
        next_id: u64,
        /// Largest stored id.
        max_id: u64,
    },
    /// Id space exhausted.
    IdExhausted,
}

impl RoadError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ZeroLength => "zero_length",
            Self::NotAxisAligned => "not_axis_aligned",
            Self::TooLong { .. } => "too_long",
            Self::InvalidWidth { .. } => "invalid_width",
            Self::CoordinateOutOfRange => "coordinate_out_of_range",
            Self::Water { .. } => "water",
            Self::SlopeTooSteep { .. } => "slope_too_steep",
            Self::TooMany => "too_many_roads",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnsupportedVersion { .. } => "unsupported_version",
            Self::DuplicateId { .. } => "duplicate_id",
            Self::InvalidRecord { .. } => "invalid_record",
            Self::InvalidNextId { .. } => "invalid_next_id",
            Self::IdExhausted => "id_exhausted",
        }
    }
}

impl fmt::Display for RoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroLength => write!(formatter, "road has zero length"),
            Self::NotAxisAligned => {
                write!(formatter, "road must be horizontal or vertical")
            }
            Self::TooLong { length } => {
                write!(
                    formatter,
                    "road is {length} m; the limit is {MAX_ROAD_LENGTH_M} m"
                )
            }
            Self::InvalidWidth { width } => write!(
                formatter,
                "road width {width} m is outside {MIN_ROAD_WIDTH_M}..={MAX_ROAD_WIDTH_M} m"
            ),
            Self::CoordinateOutOfRange => write!(formatter, "road coordinate is out of range"),
            Self::Water { x, z } => write!(formatter, "road crosses water at ({x}, {z})"),
            Self::SlopeTooSteep { rise } => write!(
                formatter,
                "road rise {rise} m exceeds the {MAX_ROAD_SLOPE_M} m limit"
            ),
            Self::TooMany => write!(formatter, "road limit is {MAX_ROADS}"),
            Self::InvalidJson(error) => write!(formatter, "invalid road JSON: {error}"),
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported road format version {version}")
            }
            Self::DuplicateId { id } => write!(formatter, "duplicate road id {id}"),
            Self::InvalidRecord { id, reason } => write!(formatter, "road {id}: {reason}"),
            Self::InvalidNextId { next_id, max_id } => write!(
                formatter,
                "next road id {next_id} is not greater than {max_id}"
            ),
            Self::IdExhausted => write!(formatter, "road id space exhausted"),
        }
    }
}

impl Error for RoadError {}

/// Roads plus the seed and id counter for one world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed these roads belong to.
    pub seed: u64,
    /// Next unused id.
    pub next_id: u64,
    /// Segments, sorted by id.
    pub roads: Vec<RoadSegment>,
}

impl RoadStore {
    /// Create an empty network.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            format_version: ROAD_FORMAT_VERSION,
            seed,
            next_id: 1,
            roads: Vec::new(),
        }
    }

    /// Validate a request against terrain without mutating the store.
    pub fn validate(
        &self,
        request: &RoadRequest,
        height_at: impl Fn(i64, i64) -> u16,
    ) -> Result<(), RoadError> {
        validate_geometry(request)?;
        if self.roads.len() >= MAX_ROADS {
            return Err(RoadError::TooMany);
        }
        validate_terrain(request, height_at)
    }

    /// Validate and append a segment.
    pub fn add(
        &mut self,
        request: &RoadRequest,
        height_at: impl Fn(i64, i64) -> u16,
    ) -> Result<RoadSegment, RoadError> {
        self.validate(request, height_at)?;
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .filter(|next| *next <= MAX_ROAD_ID)
            .ok_or(RoadError::IdExhausted)?;
        let segment = RoadSegment {
            id,
            start: request.start,
            end: request.end,
            width_m: request.width_m,
        };
        self.roads.push(segment.clone());
        Ok(segment)
    }

    /// Remove one segment by id.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.roads.len();
        self.roads.retain(|road| road.id != id);
        self.roads.len() != before
    }

    /// Remove every segment and reset the id counter.
    pub fn clear(&mut self) {
        self.roads.clear();
        self.next_id = 1;
    }

    /// Serialize the network as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, RoadError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| RoadError::InvalidJson(error.to_string()))
    }

    /// Parse and validate a network.
    pub fn from_json(json: &str) -> Result<Self, RoadError> {
        let store: Self = serde_json::from_str(json)
            .map_err(|error| RoadError::InvalidJson(error.to_string()))?;
        store.validate_loaded()?;
        Ok(store)
    }

    /// Validate loaded records without a terrain sampler.
    pub fn validate_loaded(&self) -> Result<(), RoadError> {
        if self.format_version != ROAD_FORMAT_VERSION {
            return Err(RoadError::UnsupportedVersion {
                version: self.format_version,
            });
        }
        if self.next_id == 0 || self.next_id > MAX_ROAD_ID {
            return Err(RoadError::InvalidNextId {
                next_id: self.next_id,
                max_id: MAX_ROAD_ID,
            });
        }
        if self.roads.len() > MAX_ROADS {
            return Err(RoadError::TooMany);
        }
        let mut ids = HashSet::with_capacity(self.roads.len());
        let mut max_id = 0;
        for road in &self.roads {
            if !ids.insert(road.id) {
                return Err(RoadError::DuplicateId { id: road.id });
            }
            max_id = max_id.max(road.id);
        }
        if self.next_id <= max_id {
            return Err(RoadError::InvalidNextId {
                next_id: self.next_id,
                max_id,
            });
        }
        for road in &self.roads {
            validate_record(road, self.next_id)?;
        }
        Ok(())
    }

    /// Stable FNV-1a checksum over the canonical JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let bytes = serde_json::to_vec(self).expect("road store serialization cannot fail");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

fn validate_record(road: &RoadSegment, next_id: u64) -> Result<(), RoadError> {
    if road.id == 0 || road.id > MAX_ROAD_ID || road.id >= next_id {
        return Err(RoadError::InvalidRecord {
            id: road.id,
            reason: "id is outside the JSON-safe next_id range",
        });
    }
    let request = RoadRequest {
        start: road.start,
        end: road.end,
        width_m: road.width_m,
    };
    validate_geometry(&request).map_err(|error| RoadError::InvalidRecord {
        id: road.id,
        reason: error.code(),
    })
}

fn validate_geometry(request: &RoadRequest) -> Result<(), RoadError> {
    let limit = MAX_WORLD_COORD_M as i64;
    if request
        .start
        .iter()
        .chain(request.end.iter())
        .any(|value| u64::from(value.unsigned_abs()) > limit as u64)
    {
        return Err(RoadError::CoordinateOutOfRange);
    }
    let dx = i64::from(request.end[0]) - i64::from(request.start[0]);
    let dz = i64::from(request.end[1]) - i64::from(request.start[1]);
    if (dx == 0) == (dz == 0) {
        return Err(if dx == 0 && dz == 0 {
            RoadError::ZeroLength
        } else {
            RoadError::NotAxisAligned
        });
    }
    let length = dx.abs() + dz.abs();
    if length > i64::from(MAX_ROAD_LENGTH_M) {
        return Err(RoadError::TooLong { length });
    }
    if !(MIN_ROAD_WIDTH_M..=MAX_ROAD_WIDTH_M).contains(&request.width_m) {
        return Err(RoadError::InvalidWidth {
            width: request.width_m,
        });
    }
    Ok(())
}

fn validate_terrain(
    request: &RoadRequest,
    height_at: impl Fn(i64, i64) -> u16,
) -> Result<(), RoadError> {
    let dx = (request.end[0] - request.start[0]).signum();
    let dz = (request.end[1] - request.start[1]).signum();
    let length =
        (request.end[0] - request.start[0]).abs() + (request.end[1] - request.start[1]).abs();
    let perpendicular = if dx != 0 { [0, 1] } else { [1, 0] };
    let half = i32::from(request.width_m) / 2;
    let mut min_height = u16::MAX;
    let mut max_height = 0;
    for step in 0..=length {
        for offset in [-half, 0, half] {
            let x = request.start[0] + dx * step + perpendicular[0] * offset;
            let z = request.start[1] + dz * step + perpendicular[1] * offset;
            let height = height_at(i64::from(x), i64::from(z));
            if height <= WATER_LEVEL_M {
                return Err(RoadError::Water { x, z });
            }
            min_height = min_height.min(height);
            max_height = max_height.max(height);
        }
    }
    let rise = max_height - min_height;
    if rise > MAX_ROAD_SLOPE_M {
        return Err(RoadError::SlopeTooSteep { rise });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_x: i64, _z: i64) -> u16 {
        40
    }

    fn request(axis_x: bool) -> RoadRequest {
        if axis_x {
            RoadRequest {
                start: [100, 100],
                end: [120, 100],
                width_m: 8,
            }
        } else {
            RoadRequest {
                start: [100, 100],
                end: [100, 120],
                width_m: 8,
            }
        }
    }

    #[test]
    fn adds_axis_aligned_roads_and_tracks_ids() {
        let mut store = RoadStore::new(2026);
        let first = store.add(&request(true), flat).expect("road");
        let second = store.add(&request(false), flat).expect("road");
        assert_eq!(first.id, 1);
        assert_eq!(second.id, 2);
        assert_eq!(first.length_m(), 20);
        assert_eq!(first.center(), [110.0, 100.0]);
        assert!(first.runs_along_x());
        assert!(!second.runs_along_x());
        assert_eq!(store.roads.len(), 2);
        assert_eq!(store.next_id, 3);
    }

    #[test]
    fn rejects_bad_geometry() {
        let store = RoadStore::new(1);
        let same = RoadRequest {
            start: [0, 0],
            end: [0, 0],
            width_m: 8,
        };
        assert_eq!(
            store.validate(&same, flat).expect_err("same").code(),
            "zero_length"
        );
        let diagonal = RoadRequest {
            start: [0, 0],
            end: [1, 1],
            width_m: 8,
        };
        assert_eq!(
            store
                .validate(&diagonal, flat)
                .expect_err("diagonal")
                .code(),
            "not_axis_aligned"
        );
        let wide = RoadRequest {
            start: [0, 0],
            end: [10, 0],
            width_m: 1,
        };
        assert_eq!(
            store.validate(&wide, flat).expect_err("width").code(),
            "invalid_width"
        );
        let long = RoadRequest {
            start: [0, 0],
            end: [MAX_ROAD_LENGTH_M + 1, 0],
            width_m: 8,
        };
        assert_eq!(
            store.validate(&long, flat).expect_err("long").code(),
            "too_long"
        );
    }

    #[test]
    fn rejects_water_and_steep_edges() {
        let store = RoadStore::new(1);
        let wet = |x: i64, _z: i64| if x == 105 { 20 } else { 40 };
        assert_eq!(
            store
                .validate(&request(true), wet)
                .expect_err("water")
                .code(),
            "water"
        );
        let steep = |x: i64, _z: i64| if x >= 110 { 60 } else { 40 };
        assert_eq!(
            store
                .validate(&request(true), steep)
                .expect_err("slope")
                .code(),
            "slope_too_steep"
        );
    }

    #[test]
    fn samples_the_width_edges_as_well_as_the_centreline() {
        let store = RoadStore::new(1);
        let x_road = request(true);
        let x_edge_water = |_x: i64, z: i64| if z == 104 { 20 } else { 40 };
        assert_eq!(
            store
                .validate(&x_road, x_edge_water)
                .expect_err("x edge")
                .code(),
            "water"
        );
        let z_road = request(false);
        let z_edge_water = |x: i64, _z: i64| if x == 104 { 20 } else { 40 };
        assert_eq!(
            store
                .validate(&z_road, z_edge_water)
                .expect_err("z edge")
                .code(),
            "water"
        );
    }

    #[test]
    fn rejects_extreme_coordinates_without_overflow() {
        let store = RoadStore::new(1);
        let extreme = RoadRequest {
            start: [i32::MIN, 0],
            end: [i32::MAX, 0],
            width_m: 8,
        };
        assert_eq!(
            store.validate(&extreme, flat).expect_err("extreme").code(),
            "coordinate_out_of_range"
        );
    }

    #[test]
    fn json_round_trip_and_checksum() {
        let mut store = RoadStore::new(2026);
        store.add(&request(true), flat).expect("road");
        let json = store.to_json_pretty().expect("json");
        let decoded = RoadStore::from_json(&json).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn load_rejects_duplicate_ids_and_bad_next_id() {
        let mut store = RoadStore::new(1);
        store.add(&request(true), flat).expect("road");
        let mut duplicate = store.clone();
        duplicate.roads.push(duplicate.roads[0].clone());
        assert_eq!(
            RoadStore::from_json(&duplicate.to_json_pretty().expect("json"))
                .expect_err("duplicate")
                .code(),
            "duplicate_id"
        );
        let mut bad_next = store;
        bad_next.next_id = 1;
        assert_eq!(
            RoadStore::from_json(&bad_next.to_json_pretty().expect("json"))
                .expect_err("next")
                .code(),
            "invalid_next_id"
        );
    }

    #[test]
    fn remove_and_clear_reset_ids() {
        let mut store = RoadStore::new(1);
        store.add(&request(true), flat).expect("first");
        store.add(&request(false), flat).expect("second");
        assert!(store.remove(1));
        assert_eq!(store.roads.len(), 1);
        assert!(!store.remove(99));
        store.clear();
        assert!(store.roads.is_empty());
        assert_eq!(store.next_id, 1);
    }
}
