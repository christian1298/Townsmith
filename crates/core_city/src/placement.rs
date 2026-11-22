//! Blueprint placements in the open world.
//!
//! A placement is the city-side instance of a saved `.tsbp` blueprint: where it
//! sits, how it is rotated, and the foundation height needed to put it on the
//! terrain. This module is engine-agnostic and deliberately does not depend on
//! `core_world`; callers pass a terrain-height sampler.

use std::collections::{HashMap, HashSet};
use std::fmt;

use core_structure::{StructureError, StructureSpec};
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

/// Edge length of one [`PlacementIndex`] cell, in metres.
///
/// The cell is larger than [`MAX_FOOTPRINT_M`] plus [`PLACEMENT_GAP_M`], so a
/// query only inspects a handful of neighbouring cells when testing overlap.
pub const PLACEMENT_INDEX_CELL_M: f32 = 64.0;

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
    /// Declared structural system and limits, when the blueprint has them.
    pub structure: Option<StructureSpec>,
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
    /// Declared structural system and limits; older saves have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<StructureSpec>,
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
            structure: request.structure,
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
    /// The declared structural system exceeds its limits.
    Structure(StructureError),
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
            Self::Structure(error) => error.code(),
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
            Self::Structure(error) => write!(formatter, "structural limit: {error}"),
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
    let plan = validate_geometry(request, height_at)?;

    for placement in existing {
        if footprints_overlap(
            request.center_x,
            request.center_z,
            plan.footprint_m,
            placement,
        ) {
            return Err(PlacementError::Overlap {
                other_id: placement.id,
            });
        }
    }

    Ok(plan)
}

/// Validate request fields, rotation, and terrain, but not overlap.
fn validate_geometry(
    request: &PlacementRequest,
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

    if let Some(structure) = request.structure {
        structure
            .validate_against_bounds(request.bounds_m)
            .map_err(PlacementError::Structure)?;
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

/// One placement footprint in the index grid.
#[derive(Debug, Clone, Copy)]
struct IndexEntry {
    sequence: u64,
    id: u64,
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
}

/// Derived in-memory grid index over placement footprints.
///
/// The index is not part of the save format. [`PlacementStore::rebuild_index`]
/// recreates it from the placement list, for example after a load. Every
/// placement is registered in each [`PLACEMENT_INDEX_CELL_M`] cell its
/// footprint expanded by [`PLACEMENT_GAP_M`] touches, so an overlap test only
/// inspects nearby cells instead of scanning the whole store.
///
/// The index stores its own copy of each footprint's axis-aligned bounds. That
/// keeps candidate queries independent of `PlacementStore` lookups and makes
/// `place_indexed` O(nearby placements) instead of O(n) for 250k-record cities.
#[derive(Debug, Clone, Default)]
pub struct PlacementIndex {
    cells: HashMap<(i64, i64), Vec<IndexEntry>>,
    cells_by_id: HashMap<u64, Vec<(i64, i64)>>,
    next_sequence: u64,
}

impl PlacementIndex {
    /// Create an empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of indexed placements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cells_by_id.len()
    }

    /// Whether the index holds no placements.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cells_by_id.is_empty()
    }

    /// Add a placement, replacing an older entry with the same id.
    pub fn insert(&mut self, placement: &Placement) {
        // Re-inserting an id should update its geometry, not duplicate it.
        self.remove(placement.id);

        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        let entry = IndexEntry {
            sequence,
            id: placement.id,
            min_x: placement.center[0] - placement.footprint_m[0] * 0.5,
            max_x: placement.center[0] + placement.footprint_m[0] * 0.5,
            min_z: placement.center[2] - placement.footprint_m[1] * 0.5,
            max_z: placement.center[2] + placement.footprint_m[1] * 0.5,
        };

        let (min_x, max_x, min_z, max_z) = expanded_bounds(
            placement.center[0],
            placement.center[2],
            placement.footprint_m,
        );
        let mut touched = Vec::new();
        for cell_z in cell_coord(min_z)..=cell_coord(max_z) {
            for cell_x in cell_coord(min_x)..=cell_coord(max_x) {
                let cell = (cell_x, cell_z);
                self.cells.entry(cell).or_default().push(entry);
                touched.push(cell);
            }
        }
        self.cells_by_id.insert(placement.id, touched);
    }

    /// Remove a placement by id. Returns `false` if the id is not indexed.
    pub fn remove(&mut self, id: u64) -> bool {
        let Some(cells) = self.cells_by_id.remove(&id) else {
            return false;
        };
        for cell in cells {
            let empty = if let Some(entries) = self.cells.get_mut(&cell) {
                entries.retain(|entry| entry.id != id);
                entries.is_empty()
            } else {
                false
            };
            if empty {
                self.cells.remove(&cell);
            }
        }
        true
    }

    /// Candidate placement ids near a footprint, in insertion order.
    ///
    /// The result is a superset of every placement that could overlap the
    /// requested footprint. Callers still run the exact overlap predicate, but
    /// only against these ids instead of the whole store.
    #[must_use]
    pub fn candidates(&self, center_x: f32, center_z: f32, footprint_m: [f32; 2]) -> Vec<u64> {
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        self.for_each_candidate(center_x, center_z, footprint_m, |entry| {
            if seen.insert(entry.id) {
                found.push((entry.sequence, entry.id));
            }
        });
        found.sort_unstable_by_key(|(sequence, _)| *sequence);
        found.into_iter().map(|(_, id)| id).collect()
    }

    /// First overlapping placement in store order, if any.
    ///
    /// This returns the same id as a linear scan of the store's placements:
    /// the candidate with the lowest insertion sequence that satisfies the
    /// exact [`footprints_overlap`] predicate.
    #[must_use]
    pub fn first_overlap(
        &self,
        center_x: f32,
        center_z: f32,
        footprint_m: [f32; 2],
    ) -> Option<u64> {
        let mut first: Option<(u64, u64)> = None;
        self.for_each_candidate(center_x, center_z, footprint_m, |entry| {
            if overlaps_bounds(center_x, center_z, footprint_m, entry)
                && first.is_none_or(|(sequence, _)| entry.sequence < sequence)
            {
                first = Some((entry.sequence, entry.id));
            }
        });
        first.map(|(_, id)| id)
    }

    fn for_each_candidate(
        &self,
        center_x: f32,
        center_z: f32,
        footprint_m: [f32; 2],
        mut visit: impl FnMut(&IndexEntry),
    ) {
        let (min_x, max_x, min_z, max_z) = expanded_bounds(center_x, center_z, footprint_m);
        for cell_z in cell_coord(min_z)..=cell_coord(max_z) {
            for cell_x in cell_coord(min_x)..=cell_coord(max_x) {
                if let Some(entries) = self.cells.get(&(cell_x, cell_z)) {
                    for entry in entries {
                        visit(entry);
                    }
                }
            }
        }
    }
}

fn cell_coord(value: f32) -> i64 {
    (value / PLACEMENT_INDEX_CELL_M).floor() as i64
}

/// Footprint bounds expanded by the configured gap on every side.
fn expanded_bounds(center_x: f32, center_z: f32, footprint_m: [f32; 2]) -> (f32, f32, f32, f32) {
    (
        center_x - footprint_m[0] * 0.5 - PLACEMENT_GAP_M,
        center_x + footprint_m[0] * 0.5 + PLACEMENT_GAP_M,
        center_z - footprint_m[1] * 0.5 - PLACEMENT_GAP_M,
        center_z + footprint_m[1] * 0.5 + PLACEMENT_GAP_M,
    )
}

fn overlaps_bounds(center_x: f32, center_z: f32, footprint: [f32; 2], other: &IndexEntry) -> bool {
    let a_min_x = center_x - footprint[0] * 0.5;
    let a_max_x = center_x + footprint[0] * 0.5;
    let a_min_z = center_z - footprint[1] * 0.5;
    let a_max_z = center_z + footprint[1] * 0.5;

    a_min_x < other.max_x + PLACEMENT_GAP_M
        && other.min_x < a_max_x + PLACEMENT_GAP_M
        && a_min_z < other.max_z + PLACEMENT_GAP_M
        && other.min_z < a_max_z + PLACEMENT_GAP_M
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

    /// Validate a request against the indexed neighbours only.
    ///
    /// `index` must describe the current `placements`; use
    /// [`Self::rebuild_index`] after loading or mutating the store by hand.
    /// Results are identical to [`Self::validate`], including which overlapping
    /// id is reported when several placements overlap.
    pub fn validate_indexed(
        &self,
        index: &PlacementIndex,
        request: &PlacementRequest,
        height_at: impl Fn(i64, i64) -> u16,
    ) -> Result<PlacementPlan, PlacementError> {
        let plan = validate_geometry(request, height_at)?;
        if let Some(other_id) =
            index.first_overlap(request.center_x, request.center_z, plan.footprint_m)
        {
            return Err(PlacementError::Overlap { other_id });
        }
        Ok(plan)
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

    /// Validate and append a placement, keeping the grid index in sync.
    ///
    /// The index must describe the current `placements` before the call. The
    /// result is identical to [`Self::place`].
    pub fn place_indexed(
        &mut self,
        index: &mut PlacementIndex,
        request: &PlacementRequest,
        height_at: impl Fn(i64, i64) -> u16,
    ) -> Result<Placement, PlacementError> {
        let plan = self.validate_indexed(index, request, height_at)?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(PlacementError::IdExhausted)?;
        let placement = Placement::from_request(id, request, plan);
        index.insert(&placement);
        self.placements.push(placement.clone());
        Ok(placement)
    }

    /// Remove one placement by id.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.placements.len();
        self.placements.retain(|placement| placement.id != id);
        self.placements.len() != before
    }

    /// Remove one placement and its index entry.
    pub fn remove_indexed(&mut self, index: &mut PlacementIndex, id: u64) -> bool {
        let removed = self.remove(id);
        if removed {
            index.remove(id);
        }
        removed
    }

    /// Build an index that mirrors the current placements.
    ///
    /// Call this after [`Self::from_json`] or after mutating
    /// [`Self::placements`] directly, before using the indexed operations.
    #[must_use]
    pub fn rebuild_index(&self) -> PlacementIndex {
        let mut index = PlacementIndex::new();
        for placement in &self.placements {
            index.insert(placement);
        }
        index
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
    ///
    /// This checks recorded ids, counters, geometry, stats, and optional
    /// structure metadata. It does not re-sample terrain or overlap; use a
    /// rebuild index and [`Self::validate_indexed`] for edits after a load.
    pub fn validate_loaded(&self) -> Result<(), PlacementError> {
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
    if let Some(structure) = placement.structure {
        structure
            .validate_against_bounds(placement.bounds_m)
            .map_err(PlacementError::Structure)?;
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
            structure: None,
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

    fn timber(
        system: core_structure::StructuralSystem,
        stories: u16,
        height: f32,
    ) -> StructureSpec {
        StructureSpec::new(system, stories, height, 6.0)
    }

    #[test]
    fn a_four_story_timber_building_is_rejected() {
        let mut tall = request(128.0, 128.0);
        tall.bounds_m = [8.0, 12.8, 8.0];
        tall.structure = Some(timber(
            core_structure::StructuralSystem::TimberFrame,
            4,
            12.8,
        ));
        let error = validate_placement(&tall, &[], flat).expect_err("timber limit");
        assert_eq!(error.code(), "structure_stories");
        assert!(error.to_string().contains("timber frame"));
    }

    #[test]
    fn a_four_story_brick_building_is_allowed() {
        let mut tall = request(128.0, 128.0);
        tall.bounds_m = [8.0, 12.8, 8.0];
        tall.structure = Some(timber(
            core_structure::StructuralSystem::LoadBearingBrick,
            4,
            12.8,
        ));
        validate_placement(&tall, &[], flat).expect("brick allows four stories");
    }

    #[test]
    fn a_stone_ground_floor_raises_the_timber_limit() {
        let mut composite = request(128.0, 128.0);
        composite.bounds_m = [8.0, 12.8, 8.0];
        composite.structure = Some(
            timber(core_structure::StructuralSystem::TimberFrame, 4, 12.8)
                .with_ground_system(core_structure::StructuralSystem::LoadBearingStone),
        );
        validate_placement(&composite, &[], flat).expect("stone base carries four stories");
    }

    #[test]
    fn structure_height_must_match_the_blueprint_bounds() {
        let mut mismatched = request(128.0, 128.0);
        mismatched.structure = Some(StructureSpec::new(
            core_structure::StructuralSystem::TimberFrame,
            2,
            3.0,
            0.0,
        ));
        let error = validate_placement(&mismatched, &[], flat).expect_err("height mismatch");
        assert_eq!(error.code(), "invalid_structure");
        assert!(error.to_string().contains("does not match"));
    }

    #[test]
    fn placements_without_structure_metadata_remain_compatible() {
        validate_placement(&request(128.0, 128.0), &[], flat).expect("legacy request");
    }

    #[test]
    fn loaded_placements_revalidate_their_structure() {
        let mut store = PlacementStore::new(2026);
        store.place(&request(128.0, 128.0), flat).expect("place");
        store.placements[0].structure = Some(timber(
            core_structure::StructuralSystem::TimberFrame,
            4,
            12.8,
        ));
        let error = PlacementStore::from_json(&store.to_json_pretty().expect("json"))
            .expect_err("stale structure");
        assert_eq!(error.code(), "structure_stories");
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

    fn grid_store(count: u64) -> PlacementStore {
        let mut store = PlacementStore::new(7);
        for index in 0..count {
            let x = 64.0 + (index % 13) as f32 * 14.0;
            let z = -32.0 + (index / 13) as f32 * 12.0;
            store.place(&request(x, z), flat).expect("grid placement");
        }
        store
    }

    fn linear_overlaps(
        store: &PlacementStore,
        center_x: f32,
        center_z: f32,
        footprint: [f32; 2],
    ) -> Vec<u64> {
        store
            .placements
            .iter()
            .filter(|placement| footprints_overlap(center_x, center_z, footprint, placement))
            .map(|placement| placement.id)
            .collect()
    }

    fn store_positions(store: &PlacementStore) -> HashMap<u64, usize> {
        store
            .placements
            .iter()
            .enumerate()
            .map(|(position, placement)| (placement.id, position))
            .collect()
    }

    #[test]
    fn index_candidates_cover_every_linear_overlap_and_preserve_order() {
        let store = grid_store(13 * 12);
        let index = store.rebuild_index();
        assert_eq!(index.len(), store.placements.len());
        let positions = store_positions(&store);

        let mut x = 20.0;
        while x < 300.0 {
            let mut z = -50.0;
            while z < 160.0 {
                for footprint in [[8.0, 6.0], [6.0, 8.0], [48.0, 48.0]] {
                    let expected = linear_overlaps(&store, x, z, footprint);
                    let candidates = index.candidates(x, z, footprint);
                    for id in &expected {
                        assert!(
                            candidates.contains(id),
                            "candidate set missed overlapping id {id} at ({x}, {z})"
                        );
                    }
                    assert_eq!(
                        index.first_overlap(x, z, footprint),
                        expected.first().copied(),
                        "first overlap differs from the linear scan at ({x}, {z})"
                    );

                    let candidate_positions: Vec<usize> =
                        candidates.iter().map(|id| positions[id]).collect();
                    let sorted = {
                        let mut sorted = candidate_positions.clone();
                        sorted.sort_unstable();
                        sorted
                    };
                    assert_eq!(
                        candidate_positions, sorted,
                        "candidate order is not placement order"
                    );
                }
                z += 1.7;
            }
            x += 2.3;
        }
    }

    #[test]
    fn index_handles_cell_edges_negative_coordinates_and_the_exact_gap() {
        let mut index = PlacementIndex::new();
        let first = existing(1, 64.0, 0.0);
        index.insert(&first);

        // The first footprint spans x=60..68 and crosses the x=64 cell edge.
        assert_eq!(index.candidates(72.4, 0.0, [8.0, 6.0]), vec![1]);
        assert_eq!(index.first_overlap(72.4, 0.0, [8.0, 6.0]), Some(1));
        // A second centre at 72.5 starts exactly at the 0.5 m gap.
        assert_eq!(index.first_overlap(72.5, 0.0, [8.0, 6.0]), None);
        assert_eq!(index.candidates(72.5, 0.0, [8.0, 6.0]), vec![1]);

        // The same geometry on the negative side of a cell edge.
        let negative = existing(2, -64.0, -64.0);
        index.insert(&negative);
        assert_eq!(index.first_overlap(-55.6, -64.0, [8.0, 6.0]), Some(2));
        assert_eq!(index.first_overlap(-55.5, -64.0, [8.0, 6.0]), None);

        // Cell query at an exact boundary still sees a footprint that only
        // touches it at one edge.
        let edge = existing(3, 128.0, 128.0);
        index.insert(&edge);
        assert!(index.candidates(128.0, 128.0, [8.0, 6.0]).contains(&3));
        assert!(index.candidates(133.0, 128.0, [8.0, 6.0]).contains(&3));
    }

    #[test]
    fn index_remove_and_reinsert_update_candidates() {
        let mut index = PlacementIndex::new();
        index.insert(&existing(1, 128.0, 128.0));
        index.insert(&existing(2, 200.0, 200.0));
        assert_eq!(index.len(), 2);
        assert!(index.remove(1));
        assert!(!index.remove(1));
        assert!(index.candidates(128.0, 128.0, [8.0, 6.0]).is_empty());
        assert_eq!(index.candidates(200.0, 200.0, [8.0, 6.0]), vec![2]);

        // Replacing an id moves the entry instead of duplicating it.
        index.insert(&existing(2, 128.0, 128.0));
        assert_eq!(index.len(), 1);
        assert_eq!(
            index.candidates(200.0, 200.0, [8.0, 6.0]),
            Vec::<u64>::new()
        );
        assert_eq!(index.candidates(128.0, 128.0, [8.0, 6.0]), vec![2]);
    }

    #[test]
    fn indexed_validation_matches_linear_validation_for_every_input_class() {
        let store = grid_store(30);
        let index = store.rebuild_index();
        let first = &store.placements[0];
        let first_x = first.center[0];
        let first_z = first.center[2];

        let mut overlapping = request(first_x + 1.0, first_z);
        let cases = [
            request(first_x + 1.0, first_z),
            request(first_x + 8.5, first_z),
            request(first_x + 8.51, first_z),
            request(-2_000.0, -2_000.0),
            request(1_000_001.0, 0.0),
        ];
        overlapping.rotation_deg = 45;
        let mut invalid_bounds = request(first_x, first_z);
        invalid_bounds.bounds_m = [0.0, 6.0, 6.0];

        for case in cases.iter().chain([&overlapping, &invalid_bounds]) {
            assert_eq!(
                store.validate(case, flat),
                store.validate_indexed(&index, case, flat),
                "decision differs for {case:?}"
            );
        }

        let wet = request(first_x + 1.0, first_z);
        assert_eq!(
            store.validate(&wet, |_, _| WATER_LEVEL_M),
            store.validate_indexed(&index, &wet, |_, _| WATER_LEVEL_M)
        );

        let steep = request(first_x + 1.0, first_z);
        let slope = |x: i64, _z: i64| if x % 2 == 0 { 32 } else { 35 };
        assert_eq!(
            store.validate(&steep, slope),
            store.validate_indexed(&index, &steep, slope)
        );
    }

    #[test]
    fn indexed_placement_matches_linear_ids_results_and_checksums() {
        let mut linear = PlacementStore::new(2026);
        let mut indexed = PlacementStore::new(2026);
        let mut index = PlacementIndex::new();

        for step in 0..240 {
            let x = -96.0 + (step % 17) as f32 * 8.0;
            let z = -64.0 + (step / 17) as f32 * 7.0;
            let mut case = request(x, z);
            case.rotation_deg = if step % 5 == 0 { 90 } else { 0 };

            let left = linear.place(&case, flat);
            let right = indexed.place_indexed(&mut index, &case, flat);
            assert_eq!(left, right, "placement {step} differs");
        }

        assert_eq!(linear.placements, indexed.placements);
        assert_eq!(linear.next_id, indexed.next_id);
        assert_eq!(linear.checksum(), indexed.checksum());
        assert_eq!(index.len(), indexed.placements.len());

        // Remove through both paths, then place into the freed space again.
        let removed = linear
            .placements
            .iter()
            .find(|placement| placement.id == 3)
            .cloned()
            .expect("id 3 exists");
        assert!(linear.remove(3));
        assert!(indexed.remove_indexed(&mut index, 3));
        let reopened = request(removed.center[0], removed.center[2]);
        let left = linear.place(&reopened, flat).expect("reopened space");
        let right = indexed
            .place_indexed(&mut index, &reopened, flat)
            .expect("reopened space");
        assert_eq!(left, right);
        assert_eq!(linear.placements, indexed.placements);
        assert_eq!(linear.checksum(), indexed.checksum());
    }

    #[test]
    fn rebuilt_index_matches_an_incrementally_built_index_after_load() {
        let mut original = PlacementStore::new(2026);
        let mut original_index = PlacementIndex::new();
        for step in 0..60 {
            let x = 64.0 + (step % 10) as f32 * 14.0;
            let z = 64.0 + (step / 10) as f32 * 12.0;
            original
                .place_indexed(&mut original_index, &request(x, z), flat)
                .expect("original placement");
        }

        let json = original.to_json_pretty().expect("json");
        let mut loaded = PlacementStore::from_json(&json).expect("load");
        let mut rebuilt = loaded.rebuild_index();
        assert_eq!(rebuilt.len(), original_index.len());

        let mut linear = loaded.clone();
        for step in 0..20 {
            let x = 64.0 + (step % 5) as f32 * 8.0;
            let z = 200.0 + (step / 5) as f32 * 7.0;
            let case = request(x, z);
            assert_eq!(
                linear.place(&case, flat),
                loaded.place_indexed(&mut rebuilt, &case, flat)
            );
        }
        assert_eq!(linear, loaded);
        assert_eq!(linear.checksum(), loaded.checksum());

        let reopened = request(
            linear.placements[0].center[0],
            linear.placements[0].center[2],
        );
        assert!(matches!(
            loaded.validate_indexed(&rebuilt, &reopened, flat),
            Err(PlacementError::Overlap { other_id: 1 })
        ));
    }
}
