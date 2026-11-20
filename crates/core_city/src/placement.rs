//! Blueprint placements in the open world.
//!
//! A placement is the city-side instance of a saved `.tsbp` blueprint: where it
//! sits, how it is rotated, and the foundation height needed to put it on the
//! terrain. This module is engine-agnostic and deliberately does not depend on
//! `core_world`; callers pass a terrain-height sampler.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Current world-save placement format.
pub const WORLD_SAVE_FORMAT_VERSION: u16 = 1;

/// Terrain cells at or below this height are water and cannot be built on.
pub const WATER_LEVEL_M: u16 = 24;

/// Maximum terrain elevation range under one footprint for the first slice.
pub const MAX_SLOPE_M: f32 = 2.0;

/// Minimum open space between two placement footprints.
pub const PLACEMENT_GAP_M: f32 = 0.5;

/// Maximum footprint edge accepted by the first slice (Q-010 initial limit).
pub const MAX_FOOTPRINT_M: f32 = 48.0;

/// Maximum blueprint height accepted by the first slice.
pub const MAX_BUILDING_HEIGHT_M: f32 = 512.0;

/// Largest absolute world coordinate accepted by the placement rules.
pub const MAX_WORLD_COORD_M: f32 = 1_000_000.0;

/// One request to place a blueprint in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementRequest {
    /// Stable blueprint id, for example `local:acceptance_house`.
    pub blueprint_id: String,
    /// Verified `.tsbp` content hash.
    pub blueprint_hash: String,
    /// Source package path. The first slice stores it as-is.
    pub blueprint_path: String,
    /// Human-readable blueprint name.
    pub name: String,
    /// Volume bounds in metres: width, height, depth.
    pub bounds_m: [f32; 3],
    /// Footprint centre on the terrain, in world metres.
    pub center_x: f32,
    /// Footprint centre on the terrain, in world metres.
    pub center_z: f32,
    /// Rotation around the building centre. Normalized to 0/90/180/270.
    pub rotation_deg: u16,
    /// Blueprint material cost, copied for the placement readout.
    pub material_cost: f64,
    /// Blueprint resident capacity, copied for the placement readout.
    pub resident_capacity: u32,
    /// Blueprint job slots, copied for the placement readout.
    pub job_slots: u32,
    /// Blueprint solid voxels, used as material litres by construction.
    pub solid_voxels: u64,
}

/// A placed building after validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    /// Stable placement id within the save.
    pub id: u64,
    /// Stable blueprint id.
    pub blueprint_id: String,
    /// Verified `.tsbp` content hash.
    pub blueprint_hash: String,
    /// Source package path. The first slice stores it as-is.
    pub blueprint_path: String,
    /// Human-readable blueprint name.
    pub name: String,
    /// Building base centre; `y` is the foundation top.
    pub center: [f32; 3],
    /// Normalized rotation around the building centre.
    pub rotation_deg: u16,
    /// Footprint after rotation: x, z.
    pub footprint_m: [f32; 2],
    /// Full volume bounds: width, height, depth.
    pub bounds_m: [f32; 3],
    /// Vertical distance from the lowest sampled terrain to `center.y`.
    pub foundation_m: f32,
    /// Blueprint material cost.
    pub material_cost: f64,
    /// Blueprint resident capacity.
    pub resident_capacity: u32,
    /// Blueprint job slots.
    pub job_slots: u32,
    /// Blueprint solid voxels, used as material litres by construction.
    #[serde(default)]
    pub solid_voxels: u64,
}

impl Placement {
    /// Build a placement from a validated plan.
    #[must_use]
    pub fn from_request(id: u64, request: &PlacementRequest, plan: PlacementPlan) -> Self {
        Self {
            id,
            blueprint_id: request.blueprint_id.clone(),
            blueprint_hash: request.blueprint_hash.clone(),
            blueprint_path: request.blueprint_path.clone(),
            name: request.name.clone(),
            center: plan.center,
            rotation_deg: plan.rotation_deg,
            footprint_m: plan.footprint_m,
            bounds_m: request.bounds_m,
            foundation_m: plan.foundation_m,
            material_cost: request.material_cost,
            resident_capacity: request.resident_capacity,
            job_slots: request.job_slots,
            solid_voxels: request.solid_voxels,
        }
    }
}

/// The computed geometry of a request before it is inserted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacementPlan {
    /// Building base centre.
    pub center: [f32; 3],
    /// Normalized rotation.
    pub rotation_deg: u16,
    /// Footprint after rotation.
    pub footprint_m: [f32; 2],
    /// Foundation height below the base.
    pub foundation_m: f32,
}

/// A placement validation failure.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacementError {
    /// A coordinate or bounds value is NaN or infinite.
    InvalidCoordinate {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: f32,
    },
    /// A world coordinate is outside the supported range.
    CoordinateOutOfRange {
        /// Axis name.
        axis: &'static str,
        /// Rejected value.
        value: f32,
    },
    /// A blueprint has a non-positive or non-finite dimension.
    InvalidBounds {
        /// Rejected bounds.
        bounds: [f32; 3],
    },
    /// The rotated footprint is wider or deeper than the first-slice limit.
    FootprintTooLarge {
        /// Rotated footprint.
        footprint: [f32; 2],
        /// Configured limit.
        limit: f32,
    },
    /// The blueprint is taller than the first-slice limit.
    BuildingTooTall {
        /// Rejected height.
        height: f32,
        /// Configured limit.
        limit: f32,
    },
    /// The rotation is not a multiple of 90 degrees.
    InvalidRotation {
        /// Rejected value.
        rotation: i64,
    },
    /// A copied stat is negative or non-finite.
    InvalidStats {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: f64,
    },
    /// A required blueprint identifier is missing.
    InvalidBlueprint {
        /// Field name.
        field: &'static str,
    },
    /// The footprint touches water.
    Water {
        /// Terrain cell X.
        x: i64,
        /// Terrain cell Z.
        z: i64,
        /// Terrain height.
        height: u16,
    },
    /// The terrain under the footprint is too steep.
    SlopeTooSteep {
        /// Lowest sampled terrain height.
        min: u16,
        /// Highest sampled terrain height.
        max: u16,
        /// Configured limit.
        limit: f32,
    },
    /// The footprint overlaps another placement.
    Overlap {
        /// Existing placement id.
        other_id: u64,
    },
    /// The save JSON could not be parsed.
    InvalidJson(String),
    /// The save uses an unknown format version.
    UnsupportedVersion {
        /// Rejected version.
        version: u16,
    },
    /// Two placements share an id.
    DuplicateId {
        /// Duplicated id.
        id: u64,
    },
    /// A loaded placement record is internally inconsistent.
    InvalidRecord {
        /// Placement id.
        id: u64,
        /// Short reason.
        reason: &'static str,
    },
    /// `next_id` does not leave room for the stored placements.
    InvalidNextId {
        /// Stored next id.
        next_id: u64,
        /// Largest stored placement id.
        max_id: u64,
    },
    /// The id counter is exhausted.
    IdExhausted,
}

impl PlacementError {
    /// Stable machine-readable code for the shell and smoke output.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidCoordinate { .. } => "invalid_coordinate",
            Self::CoordinateOutOfRange { .. } => "coordinate_out_of_range",
            Self::InvalidBounds { .. } => "invalid_bounds",
            Self::FootprintTooLarge { .. } => "footprint_too_large",
            Self::BuildingTooTall { .. } => "building_too_tall",
            Self::InvalidRotation { .. } => "invalid_rotation",
            Self::InvalidStats { .. } => "invalid_stats",
            Self::InvalidBlueprint { .. } => "invalid_blueprint",
            Self::Water { .. } => "water",
            Self::SlopeTooSteep { .. } => "slope_too_steep",
            Self::Overlap { .. } => "overlap",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnsupportedVersion { .. } => "unsupported_version",
            Self::DuplicateId { .. } => "duplicate_id",
            Self::InvalidRecord { .. } => "invalid_record",
            Self::InvalidNextId { .. } => "invalid_next_id",
            Self::IdExhausted => "id_exhausted",
        }
    }
}

impl fmt::Display for PlacementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCoordinate { field, value } => {
                write!(formatter, "{field} is not finite: {value}")
            }
            Self::CoordinateOutOfRange { axis, value } => {
                write!(
                    formatter,
                    "{axis} coordinate {value} is outside ±{MAX_WORLD_COORD_M} m"
                )
            }
            Self::InvalidBounds { bounds } => {
                write!(
                    formatter,
                    "blueprint bounds must be positive, got {bounds:?}"
                )
            }
            Self::FootprintTooLarge { footprint, limit } => {
                write!(
                    formatter,
                    "footprint {footprint:?} exceeds the {limit} m limit"
                )
            }
            Self::BuildingTooTall { height, limit } => {
                write!(
                    formatter,
                    "building height {height} exceeds the {limit} m limit"
                )
            }
            Self::InvalidRotation { rotation } => {
                write!(
                    formatter,
                    "rotation {rotation} is not a multiple of 90 degrees"
                )
            }
            Self::InvalidStats { field, value } => {
                write!(formatter, "{field} is invalid: {value}")
            }
            Self::InvalidBlueprint { field } => {
                write!(formatter, "blueprint {field} is empty")
            }
            Self::Water { x, z, height } => {
                write!(
                    formatter,
                    "terrain at ({x}, {z}) is water (height {height})"
                )
            }
            Self::SlopeTooSteep { min, max, limit } => {
                write!(
                    formatter,
                    "terrain rises {min}..{max} m; the limit is {limit} m"
                )
            }
            Self::Overlap { other_id } => {
                write!(formatter, "footprint overlaps placement {other_id}")
            }
            Self::InvalidJson(message) => write!(formatter, "invalid world save: {message}"),
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported world save version {version}")
            }
            Self::DuplicateId { id } => write!(formatter, "duplicate placement id {id}"),
            Self::InvalidRecord { id, reason } => {
                write!(formatter, "placement {id} is invalid: {reason}")
            }
            Self::InvalidNextId { next_id, max_id } => {
                write!(
                    formatter,
                    "next_id {next_id} must be greater than the largest id {max_id}"
                )
            }
            Self::IdExhausted => formatter.write_str("placement id counter is exhausted"),
        }
    }
}

impl std::error::Error for PlacementError {}

/// Normalize a GDScript rotation to 0/90/180/270.
pub fn normalize_rotation(rotation: i64) -> Result<u16, PlacementError> {
    if rotation.rem_euclid(90) != 0 {
        return Err(PlacementError::InvalidRotation { rotation });
    }
    Ok(rotation.rem_euclid(360) as u16)
}

/// Footprint after rotating full bounds around Y.
///
/// `rotation_deg` is expected to be normalized to 0/90/180/270; validation
/// normalizes it before calling this helper.
#[must_use]
pub fn footprint_for(bounds: [f32; 3], rotation_deg: u16) -> [f32; 2] {
    if rotation_deg.is_multiple_of(180) {
        [bounds[0], bounds[2]]
    } else {
        [bounds[2], bounds[0]]
    }
}

/// Validate a request against terrain and existing placements.
pub fn validate_placement(
    request: &PlacementRequest,
    existing: &[Placement],
    height_at: impl Fn(i64, i64) -> u16,
) -> Result<PlacementPlan, PlacementError> {
    validate_request(request)?;
    let rotation_deg = normalize_rotation(i64::from(request.rotation_deg))?;
    let footprint_m = footprint_for(request.bounds_m, rotation_deg);
    let (min_height, max_height) =
        sample_footprint(request.center_x, request.center_z, footprint_m, height_at)?;

    if f32::from(max_height - min_height) > MAX_SLOPE_M {
        return Err(PlacementError::SlopeTooSteep {
            min: min_height,
            max: max_height,
            limit: MAX_SLOPE_M,
        });
    }

    for placement in existing {
        if footprints_overlap(request.center_x, request.center_z, footprint_m, placement) {
            return Err(PlacementError::Overlap {
                other_id: placement.id,
            });
        }
    }

    Ok(PlacementPlan {
        center: [request.center_x, f32::from(max_height), request.center_z],
        rotation_deg,
        footprint_m,
        foundation_m: f32::from(max_height - min_height),
    })
}

fn validate_request(request: &PlacementRequest) -> Result<(), PlacementError> {
    for (field, value) in [
        ("blueprint_id", request.blueprint_id.as_str()),
        ("blueprint_hash", request.blueprint_hash.as_str()),
        ("blueprint_path", request.blueprint_path.as_str()),
        ("name", request.name.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(PlacementError::InvalidBlueprint { field });
        }
    }

    for value in request.bounds_m {
        if !value.is_finite() {
            return Err(PlacementError::InvalidCoordinate {
                field: "bounds",
                value,
            });
        }
    }
    if request.bounds_m.iter().any(|value| *value <= 0.0) {
        return Err(PlacementError::InvalidBounds {
            bounds: request.bounds_m,
        });
    }

    let rotation_deg = normalize_rotation(i64::from(request.rotation_deg))?;
    let footprint = footprint_for(request.bounds_m, rotation_deg);
    if footprint.iter().any(|edge| *edge > MAX_FOOTPRINT_M) {
        return Err(PlacementError::FootprintTooLarge {
            footprint,
            limit: MAX_FOOTPRINT_M,
        });
    }
    if request.bounds_m[1] > MAX_BUILDING_HEIGHT_M {
        return Err(PlacementError::BuildingTooTall {
            height: request.bounds_m[1],
            limit: MAX_BUILDING_HEIGHT_M,
        });
    }

    for (field, value) in [
        ("center_x", request.center_x),
        ("center_z", request.center_z),
    ] {
        if !value.is_finite() {
            return Err(PlacementError::InvalidCoordinate { field, value });
        }
        if value.abs() > MAX_WORLD_COORD_M {
            let axis = if field == "center_x" { "x" } else { "z" };
            return Err(PlacementError::CoordinateOutOfRange { axis, value });
        }
    }

    if !request.material_cost.is_finite() || request.material_cost < 0.0 {
        return Err(PlacementError::InvalidStats {
            field: "material_cost",
            value: request.material_cost,
        });
    }

    Ok(())
}

fn sample_footprint(
    center_x: f32,
    center_z: f32,
    footprint: [f32; 2],
    height_at: impl Fn(i64, i64) -> u16,
) -> Result<(u16, u16), PlacementError> {
    let min_x = (center_x - footprint[0] * 0.5).floor() as i64;
    let max_x = (center_x + footprint[0] * 0.5).ceil() as i64;
    let min_z = (center_z - footprint[1] * 0.5).floor() as i64;
    let max_z = (center_z + footprint[1] * 0.5).ceil() as i64;

    let mut min_height = u16::MAX;
    let mut max_height = 0;
    for z in min_z..max_z.max(min_z + 1) {
        for x in min_x..max_x.max(min_x + 1) {
            let height = height_at(x, z);
            if height <= WATER_LEVEL_M {
                return Err(PlacementError::Water { x, z, height });
            }
            min_height = min_height.min(height);
            max_height = max_height.max(height);
        }
    }
    Ok((min_height, max_height))
}

fn footprints_overlap(
    center_x: f32,
    center_z: f32,
    footprint: [f32; 2],
    other: &Placement,
) -> bool {
    let a_min_x = center_x - footprint[0] * 0.5;
    let a_max_x = center_x + footprint[0] * 0.5;
    let a_min_z = center_z - footprint[1] * 0.5;
    let a_max_z = center_z + footprint[1] * 0.5;
    let b_min_x = other.center[0] - other.footprint_m[0] * 0.5;
    let b_max_x = other.center[0] + other.footprint_m[0] * 0.5;
    let b_min_z = other.center[2] - other.footprint_m[1] * 0.5;
    let b_max_z = other.center[2] + other.footprint_m[1] * 0.5;

    a_min_x < b_max_x + PLACEMENT_GAP_M
        && b_min_x < a_max_x + PLACEMENT_GAP_M
        && a_min_z < b_max_z + PLACEMENT_GAP_M
        && b_min_z < a_max_z + PLACEMENT_GAP_M
}

/// Placements plus the world seed and id counter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementStore {
    /// Save format version.
    pub format_version: u16,
    /// World seed these placements belong to.
    pub seed: u64,
    /// Next placement id.
    pub next_id: u64,
    /// Placements in insertion order.
    pub placements: Vec<Placement>,
}

impl PlacementStore {
    /// Create an empty store for one seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self {
            format_version: WORLD_SAVE_FORMAT_VERSION,
            seed,
            next_id: 1,
            placements: Vec::new(),
        }
    }

    /// Validate a request without mutating the store.
    pub fn validate(
        &self,
        request: &PlacementRequest,
        height_at: impl Fn(i64, i64) -> u16,
    ) -> Result<PlacementPlan, PlacementError> {
        validate_placement(request, &self.placements, height_at)
    }

    /// Validate and append a placement.
    pub fn place(
        &mut self,
        request: &PlacementRequest,
        height_at: impl Fn(i64, i64) -> u16,
    ) -> Result<Placement, PlacementError> {
        let plan = self.validate(request, height_at)?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(PlacementError::IdExhausted)?;
        let placement = Placement::from_request(id, request, plan);
        self.placements.push(placement.clone());
        Ok(placement)
    }

    /// Remove one placement by id.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.placements.len();
        self.placements.retain(|placement| placement.id != id);
        self.placements.len() != before
    }

    /// Remove every placement and reset the id counter.
    pub fn clear(&mut self) {
        self.placements.clear();
        self.next_id = 1;
    }

    /// Serialize the save as pretty JSON.
    pub fn to_json_pretty(&self) -> Result<String, PlacementError> {
        serde_json::to_string_pretty(self)
            .map_err(|error| PlacementError::InvalidJson(error.to_string()))
    }

    /// Parse and validate a save.
    pub fn from_json(json: &str) -> Result<Self, PlacementError> {
        let store: Self = serde_json::from_str(json)
            .map_err(|error| PlacementError::InvalidJson(error.to_string()))?;
        store.validate_loaded()?;
        Ok(store)
    }

    /// Validate records loaded from disk without a terrain sampler.
    fn validate_loaded(&self) -> Result<(), PlacementError> {
        if self.format_version != WORLD_SAVE_FORMAT_VERSION {
            return Err(PlacementError::UnsupportedVersion {
                version: self.format_version,
            });
        }
        if self.next_id == 0 {
            return Err(PlacementError::InvalidNextId {
                next_id: self.next_id,
                max_id: 0,
            });
        }

        let mut ids = HashSet::with_capacity(self.placements.len());
        let mut max_id = 0;
        for placement in &self.placements {
            if !ids.insert(placement.id) {
                return Err(PlacementError::DuplicateId { id: placement.id });
            }
            max_id = max_id.max(placement.id);
            validate_record(placement, self.next_id)?;
        }
        if self.next_id <= max_id {
            return Err(PlacementError::InvalidNextId {
                next_id: self.next_id,
                max_id,
            });
        }
        Ok(())
    }

    /// Stable checksum of the save contents.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

fn validate_record(placement: &Placement, next_id: u64) -> Result<(), PlacementError> {
    if placement.id == 0 || placement.id >= next_id {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "id is outside the next_id range",
        });
    }
    for value in [
        placement.blueprint_id.as_str(),
        placement.blueprint_hash.as_str(),
        placement.blueprint_path.as_str(),
        placement.name.as_str(),
    ] {
        if value.trim().is_empty() {
            return Err(PlacementError::InvalidRecord {
                id: placement.id,
                reason: "missing blueprint identifier",
            });
        }
    }

    for value in placement.bounds_m {
        if !value.is_finite() || value <= 0.0 {
            return Err(PlacementError::InvalidRecord {
                id: placement.id,
                reason: "invalid bounds",
            });
        }
    }
    let rotation = normalize_rotation(i64::from(placement.rotation_deg)).map_err(|_| {
        PlacementError::InvalidRecord {
            id: placement.id,
            reason: "invalid rotation",
        }
    })?;
    if rotation != placement.rotation_deg {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "rotation is not normalized",
        });
    }
    let expected_footprint = footprint_for(placement.bounds_m, rotation);
    if (placement.footprint_m[0] - expected_footprint[0]).abs() > 1.0e-3
        || (placement.footprint_m[1] - expected_footprint[1]).abs() > 1.0e-3
    {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "footprint does not match bounds and rotation",
        });
    }
    if expected_footprint
        .iter()
        .any(|edge| *edge > MAX_FOOTPRINT_M)
    {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "footprint exceeds the first-slice limit",
        });
    }
    if placement.bounds_m[1] > MAX_BUILDING_HEIGHT_M {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "height exceeds the first-slice limit",
        });
    }
    if !placement.foundation_m.is_finite()
        || placement.foundation_m < 0.0
        || placement.foundation_m > MAX_SLOPE_M + 1.0e-3
    {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "invalid foundation height",
        });
    }
    if placement.center.iter().any(|value| !value.is_finite()) {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "invalid centre",
        });
    }
    if placement.center[1] < 0.0 {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "centre is below the world",
        });
    }
    if placement.center[0].abs() > MAX_WORLD_COORD_M
        || placement.center[2].abs() > MAX_WORLD_COORD_M
    {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "centre is outside the world range",
        });
    }
    if !placement.material_cost.is_finite() || placement.material_cost < 0.0 {
        return Err(PlacementError::InvalidRecord {
            id: placement.id,
            reason: "invalid material cost",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(x: f32, z: f32) -> PlacementRequest {
        PlacementRequest {
            blueprint_id: "local:house".to_string(),
            blueprint_hash: "blake3:test".to_string(),
            blueprint_path: "/tmp/house.tsbp".to_string(),
            name: "House".to_string(),
            bounds_m: [8.0, 6.0, 6.0],
            center_x: x,
            center_z: z,
            rotation_deg: 0,
            material_cost: 100.0,
            resident_capacity: 2,
            job_slots: 0,
            solid_voxels: 100,
        }
    }

    fn existing(id: u64, x: f32, z: f32) -> Placement {
        let request = request(x, z);
        let plan = PlacementPlan {
            center: [x, 32.0, z],
            rotation_deg: 0,
            footprint_m: [8.0, 6.0],
            foundation_m: 0.0,
        };
        Placement::from_request(id, &request, plan)
    }

    fn flat(_x: i64, _z: i64) -> u16 {
        32
    }

    #[test]
    fn flat_site_places_at_terrain_height() {
        let plan = validate_placement(&request(128.0, 128.0), &[], flat).expect("place");
        assert_eq!(plan.center, [128.0, 32.0, 128.0]);
        assert_eq!(plan.foundation_m, 0.0);
        assert_eq!(plan.footprint_m, [8.0, 6.0]);
    }

    #[test]
    fn rotation_swaps_the_footprint() {
        let mut rotated = request(128.0, 128.0);
        rotated.rotation_deg = 90;
        let plan = validate_placement(&rotated, &[], flat).expect("place");
        assert_eq!(plan.rotation_deg, 90);
        assert_eq!(plan.footprint_m, [6.0, 8.0]);
    }

    #[test]
    fn rotation_is_normalized() {
        assert_eq!(normalize_rotation(-90).expect("rotation"), 270);
        assert_eq!(normalize_rotation(450).expect("rotation"), 90);
        assert_eq!(normalize_rotation(720).expect("rotation"), 0);
        assert!(matches!(
            normalize_rotation(45),
            Err(PlacementError::InvalidRotation { rotation: 45 })
        ));
    }

    #[test]
    fn water_is_rejected() {
        let error = validate_placement(&request(128.0, 128.0), &[], |_, _| WATER_LEVEL_M)
            .expect_err("water");
        assert_eq!(error.code(), "water");
    }

    #[test]
    fn steep_slope_is_rejected() {
        let error = validate_placement(
            &request(128.0, 128.0),
            &[],
            |x, _| {
                if x >= 128 {
                    35
                } else {
                    32
                }
            },
        )
        .expect_err("slope");
        assert_eq!(error.code(), "slope_too_steep");
    }

    #[test]
    fn overlap_is_rejected_but_a_half_metre_gap_is_allowed() {
        let first = existing(1, 128.0, 128.0);

        let overlap =
            validate_placement(&request(130.0, 128.0), std::slice::from_ref(&first), flat)
                .expect_err("overlap");
        assert!(matches!(overlap, PlacementError::Overlap { other_id: 1 }));

        // First footprint ends at x=132; a second centre at 136.5 starts at
        // 132.5, exactly the configured gap.
        validate_placement(&request(136.5, 128.0), &[first], flat).expect("gap allowed");
    }

    #[test]
    fn invalid_requests_are_rejected() {
        let mut bounds = request(128.0, 128.0);
        bounds.bounds_m = [0.0, 6.0, 6.0];
        assert_eq!(
            validate_placement(&bounds, &[], flat)
                .expect_err("bounds")
                .code(),
            "invalid_bounds"
        );

        let mut rotation = request(128.0, 128.0);
        rotation.rotation_deg = 45;
        assert_eq!(
            validate_placement(&rotation, &[], flat)
                .expect_err("rotation")
                .code(),
            "invalid_rotation"
        );

        let mut far = request(128.0, 128.0);
        far.center_x = 2.0e6;
        assert_eq!(
            validate_placement(&far, &[], flat)
                .expect_err("range")
                .code(),
            "coordinate_out_of_range"
        );

        let mut cost = request(128.0, 128.0);
        cost.material_cost = f64::NAN;
        assert_eq!(
            validate_placement(&cost, &[], flat)
                .expect_err("cost")
                .code(),
            "invalid_stats"
        );
    }

    #[test]
    fn store_places_and_assigns_ids() {
        let mut store = PlacementStore::new(7);
        let first = store.place(&request(128.0, 128.0), flat).expect("first");
        assert_eq!(first.id, 1);
        assert_eq!(store.next_id, 2);

        let second = store.place(&request(136.5, 128.0), flat).expect("second");
        assert_eq!(second.id, 2);
        assert_eq!(store.placements.len(), 2);
    }

    #[test]
    fn json_round_trip_preserves_checksum_and_statistics() {
        let mut store = PlacementStore::new(2026);
        store.place(&request(128.0, 128.0), flat).expect("place");
        let checksum = store.checksum();
        let json = store.to_json_pretty().expect("json");
        let loaded = PlacementStore::from_json(&json).expect("load");
        assert_eq!(loaded, store);
        assert_eq!(loaded.checksum(), checksum);
        assert_eq!(loaded.placements[0].resident_capacity, 2);
        assert_eq!(loaded.placements[0].material_cost, 100.0);
    }

    #[test]
    fn load_rejects_unknown_versions_and_bad_counters() {
        let store = PlacementStore::new(2026);
        let json = store.to_json_pretty().expect("json");

        let newer = json.replace("\"format_version\": 1", "\"format_version\": 2");
        assert_eq!(
            PlacementStore::from_json(&newer)
                .expect_err("version")
                .code(),
            "unsupported_version"
        );

        let bad_next = json.replace("\"next_id\": 1", "\"next_id\": 0");
        assert_eq!(
            PlacementStore::from_json(&bad_next)
                .expect_err("next")
                .code(),
            "invalid_next_id"
        );
    }

    #[test]
    fn load_rejects_duplicate_ids_and_bad_footprints() {
        let mut store = PlacementStore::new(2026);
        store.place(&request(128.0, 128.0), flat).expect("place");
        let mut duplicate = store.clone();
        duplicate.placements.push(duplicate.placements[0].clone());
        assert_eq!(
            PlacementStore::from_json(&duplicate.to_json_pretty().expect("json"))
                .expect_err("duplicate")
                .code(),
            "duplicate_id"
        );

        let mut bad_footprint = store;
        bad_footprint.placements[0].footprint_m = [99.0, 99.0];
        assert_eq!(
            PlacementStore::from_json(&bad_footprint.to_json_pretty().expect("json"))
                .expect_err("footprint")
                .code(),
            "invalid_record"
        );
    }

    #[test]
    fn load_rejects_inconsistent_geometry_and_stats() {
        let mut store = PlacementStore::new(2026);
        store.place(&request(128.0, 128.0), flat).expect("place");

        let mut deep = store.clone();
        deep.placements[0].foundation_m = MAX_SLOPE_M + 1.0;
        assert_eq!(
            PlacementStore::from_json(&deep.to_json_pretty().expect("json"))
                .expect_err("foundation")
                .code(),
            "invalid_record"
        );

        let mut below = store.clone();
        below.placements[0].center[1] = -1.0;
        assert_eq!(
            PlacementStore::from_json(&below.to_json_pretty().expect("json"))
                .expect_err("centre")
                .code(),
            "invalid_record"
        );

        let mut tall = store;
        tall.placements[0].bounds_m[1] = MAX_BUILDING_HEIGHT_M + 1.0;
        assert_eq!(
            PlacementStore::from_json(&tall.to_json_pretty().expect("json"))
                .expect_err("height")
                .code(),
            "invalid_record"
        );
    }

    #[test]
    fn remove_and_clear_update_the_store() {
        let mut store = PlacementStore::new(7);
        store.place(&request(128.0, 128.0), flat).expect("first");
        store.place(&request(136.5, 128.0), flat).expect("second");
        assert!(store.remove(1));
        assert_eq!(store.placements.len(), 1);
        assert!(!store.remove(99));
        store.clear();
        assert!(store.placements.is_empty());
        assert_eq!(store.next_id, 1);
    }
}
