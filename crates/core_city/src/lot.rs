//! Zoned lots on the 1 m terrain grid.
//!
//! A lot is an axis-aligned parcel with a zone. This slice validates terrain,
//! overlap, and road frontage; it does not grow buildings or assign ownership.
//! Lots are deliberately separate from placements: a placed blueprint can
//! exist before the settlement layer locks it to a parcel.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::placement::WATER_LEVEL_M;
use crate::road::RoadStore;

/// Current lot save format.
pub const LOT_FORMAT_VERSION: u16 = 1;

/// Shortest side of a lot in metres.
pub const MIN_LOT_SIDE_M: i32 = 4;

/// Longest side of a lot in metres.
pub const MAX_LOT_SIDE_M: i32 = 100;

/// Smallest allowed lot area in square metres.
pub const MIN_LOT_AREA_M2: i32 = 16;

/// Largest allowed lot area in square metres.
pub const MAX_LOT_AREA_M2: i32 = 10_000;

/// Maximum distance from a lot edge to a road corridor.
pub const LOT_ROAD_GAP_M: i32 = 2;

/// Maximum terrain rise/fall across a lot.
pub const MAX_LOT_SLOPE_M: u16 = 2;

/// Hard cap for one save.
pub const MAX_LOTS: usize = 100_000;

/// Largest id representable by the GDScript bridge.
pub const MAX_LOT_ID: u64 = i64::MAX as u64;

const MAX_LOT_COORD: i32 = 1_000_000;

/// Zoning class for a parcel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    /// Housing.
    Residential,
    /// Shops, offices, and services.
    Commercial,
    /// Workshops, warehouses, and industry.
    Industrial,
}

impl Zone {
    /// Every zone in a stable order.
    pub const ALL: [Self; 3] = [Self::Residential, Self::Commercial, Self::Industrial];

    /// Stable lowercase name used by JSON, the bridge, and docs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Residential => "residential",
            Self::Commercial => "commercial",
            Self::Industrial => "industrial",
        }
    }

    /// Parse a bridge/doc string.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "residential" => Some(Self::Residential),
            "commercial" => Some(Self::Commercial),
            "industrial" => Some(Self::Industrial),
            _ => None,
        }
    }
}

/// One zoned parcel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lot {
    /// Stable id within the world.
    pub id: u64,
    /// Bounds `[min_x, min_z, max_x, max_z]` in metres.
    pub bounds: [i32; 4],
    /// Zoning class.
    pub zone: Zone,
}

impl Lot {
    /// Width in metres.
    #[must_use]
    pub const fn width_m(&self) -> i32 {
        self.bounds[2] - self.bounds[0]
    }

    /// Depth in metres.
    #[must_use]
    pub const fn depth_m(&self) -> i32 {
        self.bounds[3] - self.bounds[1]
    }

    /// Area in square metres.
    #[must_use]
    pub const fn area_m2(&self) -> i32 {
        self.width_m() * self.depth_m()
    }

    /// Centre in the horizontal plane.
    #[must_use]
    pub fn center(&self) -> [f32; 2] {
        [
            (self.bounds[0] + self.bounds[2]) as f32 * 0.5,
            (self.bounds[1] + self.bounds[3]) as f32 * 0.5,
        ]
    }
}

/// A request to add one lot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LotRequest {
    /// Bounds `[min_x, min_z, max_x, max_z]` in metres.
    pub bounds: [i32; 4],
    /// Zoning class.
    pub zone: Zone,
}

/// Lot validation and persistence errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LotError {
    /// Bounds are empty or inverted.
    InvalidBounds,
    /// A side is outside the allowed range.
    InvalidSide {
        /// Requested side length in metres.
        side: i32,
    },
    /// Area is outside the allowed range.
    InvalidArea {
        /// Requested area in square metres.
        area: i32,
    },
    /// A coordinate is outside the world range.
    CoordinateOutOfRange,
    /// A sampled lot cell is at or below water.
    Water {
        /// Cell X.
        x: i32,
        /// Cell Z.
        z: i32,
    },
    /// Terrain rise/fall across the lot exceeds the limit.
    SlopeTooSteep {
        /// Sampled rise in metres.
        rise: u16,
    },
    /// The lot overlaps an existing lot.
    Overlap {
        /// Existing lot id.
        id: u64,
    },
    /// No road is close enough to the lot edge.
    NoRoadFrontage,
    /// The store is full.
    TooMany,
    /// JSON could not be parsed.
    InvalidJson(String),
    /// The save format version is not supported.
    UnsupportedVersion {
        /// Version found in the file.
        version: u16,
    },
    /// Two lots share an id.
    DuplicateId {
        /// Duplicate id.
        id: u64,
    },
    /// A record is structurally invalid.
    InvalidRecord {
        /// Record id.
        id: u64,
        /// Human-readable reason.
        reason: String,
    },
    /// `next_id` is zero or not greater than every stored id.
    InvalidNextId {
        /// Stored next id.
        next_id: u64,
        /// Largest stored id.
        max_id: u64,
    },
    /// The id space is exhausted.
    IdExhausted,
}

impl LotError {
    /// Stable machine-readable code for the bridge.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidBounds => "invalid_bounds",
            Self::InvalidSide { .. } => "invalid_side",
            Self::InvalidArea { .. } => "invalid_area",
            Self::CoordinateOutOfRange => "coordinate_out_of_range",
            Self::Water { .. } => "water",
            Self::SlopeTooSteep { .. } => "slope_too_steep",
            Self::Overlap { .. } => "overlap",
            Self::NoRoadFrontage => "no_road_frontage",
            Self::TooMany => "too_many",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnsupportedVersion { .. } => "unsupported_version",
            Self::DuplicateId { .. } => "duplicate_id",
            Self::InvalidRecord { .. } => "invalid_record",
            Self::InvalidNextId { .. } => "invalid_next_id",
            Self::IdExhausted => "id_exhausted",
        }
    }
}

impl fmt::Display for LotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBounds => write!(formatter, "lot bounds are empty or inverted"),
            Self::InvalidSide { side } => write!(
                formatter,
                "lot side {side} m is outside {MIN_LOT_SIDE_M}..={MAX_LOT_SIDE_M} m"
            ),
            Self::InvalidArea { area } => write!(
                formatter,
                "lot area {area} m² is outside {MIN_LOT_AREA_M2}..={MAX_LOT_AREA_M2} m²"
            ),
            Self::CoordinateOutOfRange => write!(formatter, "lot coordinate is out of range"),
            Self::Water { x, z } => write!(formatter, "lot crosses water at ({x}, {z})"),
            Self::SlopeTooSteep { rise } => write!(
                formatter,
                "lot rise {rise} m exceeds the {MAX_LOT_SLOPE_M} m limit"
            ),
            Self::Overlap { id } => write!(formatter, "lot overlaps lot {id}"),
            Self::NoRoadFrontage => write!(formatter, "lot has no road frontage"),
            Self::TooMany => write!(formatter, "lot limit is {MAX_LOTS}"),
            Self::InvalidJson(error) => write!(formatter, "invalid lot JSON: {error}"),
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported lot format version {version}")
            }
            Self::DuplicateId { id } => write!(formatter, "duplicate lot id {id}"),
            Self::InvalidRecord { id, reason } => write!(formatter, "lot {id}: {reason}"),
            Self::InvalidNextId { next_id, max_id } => write!(
                formatter,
                "next lot id {next_id} is not greater than {max_id}"
            ),
            Self::IdExhausted => write!(formatter, "lot id space exhausted"),
        }
    }
}

impl Error for LotError {}

/// Lots plus the seed and id counter for one world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LotStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed these lots belong to.
    pub seed: u64,
    /// Next unused id.
    pub next_id: u64,
    /// Lots, sorted by id.
    pub lots: Vec<Lot>,
}

impl LotStore {
    /// Create an empty lot store.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            format_version: LOT_FORMAT_VERSION,
            seed,
            next_id: 1,
            lots: Vec::new(),
        }
    }

    /// Validate a request without mutating the store.
    pub fn validate(
        &self,
        request: &LotRequest,
        height_at: impl Fn(i64, i64) -> u16,
        roads: &RoadStore,
    ) -> Result<(), LotError> {
        validate_geometry(request)?;
        if self.lots.len() >= MAX_LOTS {
            return Err(LotError::TooMany);
        }
        if let Some(existing) = self
            .lots
            .iter()
            .find(|lot| overlaps(lot.bounds, request.bounds))
        {
            return Err(LotError::Overlap { id: existing.id });
        }
        if !roads.has_frontage(request.bounds, LOT_ROAD_GAP_M) {
            return Err(LotError::NoRoadFrontage);
        }
        validate_terrain(request.bounds, height_at)?;
        Ok(())
    }

    /// Validate and append a lot.
    pub fn add(
        &mut self,
        request: &LotRequest,
        height_at: impl Fn(i64, i64) -> u16,
        roads: &RoadStore,
    ) -> Result<Lot, LotError> {
        self.validate(request, height_at, roads)?;
        let id = self.next_id;
        // Assignable ids are `1..i64::MAX`. The ceiling check consumes the
        // final slot: when `next_id == MAX_LOT_ID` no id is pushed and
        // `IdExhausted` is reported, so `i64::MAX` itself is never assigned.
        self.next_id = id
            .checked_add(1)
            .filter(|next| *next <= MAX_LOT_ID)
            .ok_or(LotError::IdExhausted)?;
        let lot = Lot {
            id,
            bounds: request.bounds,
            zone: request.zone,
        };
        self.lots.push(lot.clone());
        Ok(lot)
    }

    /// Remove one lot by id.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.lots.len();
        self.lots.retain(|lot| lot.id != id);
        self.lots.len() != before
    }

    /// Remove every lot and reset the id counter.
    pub fn clear(&mut self) {
        self.lots.clear();
        self.next_id = 1;
    }

    /// Serialize the lots as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, LotError> {
        serde_json::to_string_pretty(self).map_err(|error| LotError::InvalidJson(error.to_string()))
    }

    /// Parse and validate a lot store.
    pub fn from_json(json: &str) -> Result<Self, LotError> {
        let store: Self =
            serde_json::from_str(json).map_err(|error| LotError::InvalidJson(error.to_string()))?;
        store.validate_loaded()?;
        Ok(store)
    }

    /// Validate loaded records without a terrain or road sampler.
    pub fn validate_loaded(&self) -> Result<(), LotError> {
        if self.format_version != LOT_FORMAT_VERSION {
            return Err(LotError::UnsupportedVersion {
                version: self.format_version,
            });
        }
        if self.next_id == 0 || self.next_id > MAX_LOT_ID {
            return Err(LotError::InvalidNextId {
                next_id: self.next_id,
                max_id: MAX_LOT_ID,
            });
        }
        if self.lots.len() > MAX_LOTS {
            return Err(LotError::TooMany);
        }
        let mut ids = HashSet::with_capacity(self.lots.len());
        let mut max_id = 0;
        for lot in &self.lots {
            if !ids.insert(lot.id) {
                return Err(LotError::DuplicateId { id: lot.id });
            }
            max_id = max_id.max(lot.id);
            validate_record(lot, self.next_id)?;
        }
        if self.next_id <= max_id {
            return Err(LotError::InvalidNextId {
                next_id: self.next_id,
                max_id,
            });
        }
        Ok(())
    }

    /// Stable FNV-1a checksum over the canonical JSON.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let bytes = serde_json::to_vec(self).expect("lot store serialization cannot fail");
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

fn validate_record(lot: &Lot, next_id: u64) -> Result<(), LotError> {
    if lot.id == 0 || lot.id > MAX_LOT_ID || lot.id >= next_id {
        return Err(LotError::InvalidRecord {
            id: lot.id,
            reason: "id is outside the JSON-safe next_id range".to_string(),
        });
    }
    let request = LotRequest {
        bounds: lot.bounds,
        zone: lot.zone,
    };
    validate_geometry(&request).map_err(|error| LotError::InvalidRecord {
        id: lot.id,
        reason: error.to_string(),
    })
}

fn validate_geometry(request: &LotRequest) -> Result<(), LotError> {
    validate_bounds(request.bounds)?;
    let width = request.bounds[2] - request.bounds[0];
    let depth = request.bounds[3] - request.bounds[1];
    for side in [width, depth] {
        if !(MIN_LOT_SIDE_M..=MAX_LOT_SIDE_M).contains(&side) {
            return Err(LotError::InvalidSide { side });
        }
    }
    let area = width * depth;
    if !(MIN_LOT_AREA_M2..=MAX_LOT_AREA_M2).contains(&area) {
        return Err(LotError::InvalidArea { area });
    }
    Ok(())
}

fn validate_bounds(bounds: [i32; 4]) -> Result<(), LotError> {
    let [min_x, min_z, max_x, max_z] = bounds;
    if min_x >= max_x || min_z >= max_z {
        return Err(LotError::InvalidBounds);
    }
    for value in [min_x, min_z, max_x, max_z] {
        if value.abs() > MAX_LOT_COORD {
            return Err(LotError::CoordinateOutOfRange);
        }
    }
    Ok(())
}

fn validate_terrain(bounds: [i32; 4], height_at: impl Fn(i64, i64) -> u16) -> Result<(), LotError> {
    let [min_x, min_z, max_x, max_z] = bounds;
    let mut min_height = u16::MAX;
    let mut max_height = 0;
    for z in axis_points(min_z, max_z) {
        for x in axis_points(min_x, max_x) {
            let height = height_at(i64::from(x), i64::from(z));
            if height <= WATER_LEVEL_M {
                return Err(LotError::Water { x, z });
            }
            min_height = min_height.min(height);
            max_height = max_height.max(height);
        }
    }
    let rise = max_height - min_height;
    if rise > MAX_LOT_SLOPE_M {
        return Err(LotError::SlopeTooSteep { rise });
    }
    Ok(())
}

fn axis_points(start: i32, end: i32) -> Vec<i32> {
    // Lot bounds are half-open, so sample `[start, end)` and include the last
    // cell inside the parcel.
    let mut points: Vec<i32> = (start..end).step_by(2).collect();
    let last = end - 1;
    if points.last() != Some(&last) {
        points.push(last);
    }
    points
}

fn overlaps(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::road::RoadRequest;

    fn roads() -> RoadStore {
        let mut store = RoadStore::new(2026);
        store
            .add(
                &RoadRequest {
                    start: [100, 120],
                    end: [140, 120],
                    width_m: 8,
                },
                |_, _| 40,
            )
            .expect("road");
        store
    }

    fn flat_lot() -> LotRequest {
        LotRequest {
            bounds: [110, 124, 130, 144],
            zone: Zone::Residential,
        }
    }

    #[test]
    fn adds_a_lot_with_road_frontage() {
        let mut store = LotStore::new(2026);
        let lot = store.add(&flat_lot(), |_, _| 40, &roads()).expect("lot");
        assert_eq!(lot.id, 1);
        assert_eq!(lot.width_m(), 20);
        assert_eq!(lot.depth_m(), 20);
        assert_eq!(lot.area_m2(), 400);
        assert_eq!(lot.center(), [120.0, 134.0]);
        assert_eq!(store.lots.len(), 1);
    }

    #[test]
    fn rejects_a_lot_without_road_frontage() {
        let mut store = LotStore::new(2026);
        let request = LotRequest {
            bounds: [200, 200, 220, 220],
            zone: Zone::Commercial,
        };
        assert_eq!(
            store.add(&request, |_, _| 40, &roads()),
            Err(LotError::NoRoadFrontage)
        );
    }

    #[test]
    fn rejects_water_and_steep_terrain() {
        let mut store = LotStore::new(2026);
        let water = |x: i64, z: i64| if x == 120 && z == 130 { 24 } else { 40 };
        assert_eq!(
            store.add(&flat_lot(), water, &roads()),
            Err(LotError::Water { x: 120, z: 130 })
        );
        // `max_x` is outside the half-open parcel.
        let max_edge_water = |x: i64, _z: i64| if x == 130 { 24 } else { 40 };
        assert!(store.add(&flat_lot(), max_edge_water, &roads()).is_ok());
        let mut steep_store = LotStore::new(2026);
        let steep = |x: i64, _z: i64| if x >= 125 { 43 } else { 40 };
        assert_eq!(
            steep_store.add(&flat_lot(), steep, &roads()),
            Err(LotError::SlopeTooSteep { rise: 3 })
        );
    }

    #[test]
    fn rejects_overlap_but_allows_shared_edges() {
        let mut store = LotStore::new(2026);
        store.add(&flat_lot(), |_, _| 40, &roads()).expect("first");
        let overlapping = LotRequest {
            bounds: [120, 124, 140, 144],
            zone: Zone::Industrial,
        };
        assert_eq!(
            store.add(&overlapping, |_, _| 40, &roads()),
            Err(LotError::Overlap { id: 1 })
        );
        let adjacent = LotRequest {
            bounds: [130, 124, 150, 144],
            zone: Zone::Industrial,
        };
        assert!(store.add(&adjacent, |_, _| 40, &roads()).is_ok());
    }

    #[test]
    fn json_round_trip_and_checksum_are_stable() {
        let mut store = LotStore::new(2026);
        store.add(&flat_lot(), |_, _| 40, &roads()).expect("lot");
        let encoded = store.to_json_pretty().expect("encode");
        let decoded = LotStore::from_json(&encoded).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.checksum(), store.checksum());
    }

    #[test]
    fn zones_round_trip_through_json_names() {
        for zone in Zone::ALL {
            let encoded = serde_json::to_string(&zone).expect("zone");
            assert_eq!(Zone::parse(zone.as_str()), Some(zone));
            assert!(encoded.contains(zone.as_str()));
        }
    }

    #[test]
    fn rejects_invalid_bounds_and_ids() {
        let mut store = LotStore::new(2026);
        let inverted = LotRequest {
            bounds: [10, 10, 10, 20],
            zone: Zone::Residential,
        };
        assert_eq!(
            store.add(&inverted, |_, _| 40, &roads()),
            Err(LotError::InvalidBounds)
        );
        let mut encoded = serde_json::to_value(store).expect("json");
        encoded["next_id"] = serde_json::json!(0);
        let parsed: LotStore = serde_json::from_value(encoded).expect("store");
        assert!(matches!(
            parsed.validate_loaded(),
            Err(LotError::InvalidNextId { .. })
        ));
    }
}
