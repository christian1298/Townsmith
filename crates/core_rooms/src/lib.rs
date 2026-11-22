//! Room detection v0 and gameplay stats for Townsmith.
//!
//! Detection is deliberately simple: for each 3 m floor band it flood-fills
//! air on one representative horizontal layer, ignores components that touch
//! the volume boundary (exterior air), and treats every remaining component as
//! a room. Door and window material voxels adjacent to the component are
//! counted, and functional props assign a room type, bed capacity, and job
//! slots.
//!
//! The crate is engine-agnostic. It depends only on [`core_voxel`] and returns
//! plain Rust data; a shell maps [`RoomRecord`] to
//! `core_blueprint::RoomRecord` without creating a dependency from this crate
//! back into the package format.
//!
//! # Example
//!
//! ```
//! use core_rooms::{detect_rooms, RoomMaterials};
//! use core_voxel::VoxelVolume;
//!
//! let volume = VoxelVolume::default();
//! let materials = RoomMaterials::new(1, 2, 3, 4, 5);
//! let stats = detect_rooms(&volume, &materials, &[]);
//!
//! assert!(stats.rooms.is_empty());
//! assert_eq!(stats.solid_voxels, 0);
//! assert_eq!(stats.class, "lower_middle");
//! ```

#![forbid(unsafe_code)]

pub mod demo;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use core_voxel::{MaterialId, VolumePos, VoxelVolume, AIR, VOXEL_SIZE_M};
use serde::{Deserialize, Serialize};

/// Nominal height of one detection band in metres.
///
/// The final band is partial when the volume height is not a multiple of
/// [`FLOOR_BAND_VOXELS`]; per-room volume uses the actual band height instead
/// of this nominal value.
pub const FLOOR_BAND_M: f32 = 3.0;

/// Height of one detection band in 10 cm voxels.
pub const FLOOR_BAND_VOXELS: u32 = 30;

/// Area of one 10 cm voxel in square metres.
pub const VOXEL_AREA_M2: f32 = VOXEL_SIZE_M * VOXEL_SIZE_M;

/// Materials that bound a room.
///
/// Detection v0 uses [`Self::door`] and [`Self::window`] for room stats.
/// [`Self::wall`], [`Self::floor`], and [`Self::ceiling`] are carried so
/// structural checks can use the same input contract later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomMaterials {
    /// Wall material id.
    pub wall: MaterialId,
    /// Floor material id.
    pub floor: MaterialId,
    /// Ceiling/roof material id.
    pub ceiling: MaterialId,
    /// Door material id.
    pub door: MaterialId,
    /// Window/glass material id.
    pub window: MaterialId,
}

impl RoomMaterials {
    /// Create a material set.
    #[must_use]
    pub const fn new(
        wall: MaterialId,
        floor: MaterialId,
        ceiling: MaterialId,
        door: MaterialId,
        window: MaterialId,
    ) -> Self {
        Self {
            wall,
            floor,
            ceiling,
            door,
            window,
        }
    }
}

/// Functional prop kinds used by room inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PropKind {
    /// A bed; adds resident capacity and implies a bedroom.
    Bed,
    /// A stove or oven; implies a kitchen.
    Stove,
    /// A toilet; implies a bathroom.
    Toilet,
    /// A shower; implies a bathroom.
    Shower,
    /// A sink. Recognized but does not classify a room by itself.
    Sink,
    /// A table; implies living/dining space.
    Table,
    /// A sofa; implies living/dining space.
    Sofa,
    /// A desk; adds a job slot and implies an office.
    Desk,
    /// Any other prop. Recognized but ignored by inference.
    Other,
}

/// One prop placed in a volume, in metres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropInput {
    /// Content id, for example `base:bed_single`.
    pub id: String,
    /// Functional kind.
    pub kind: PropKind,
    /// Position in volume metres.
    pub position: [f32; 3],
}

impl PropInput {
    /// Create a prop input.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: PropKind, position: [f32; 3]) -> Self {
        Self {
            id: id.into(),
            kind,
            position,
        }
    }
}

/// One detected room.
///
/// Fields mirror `core_blueprint::RoomRecord` so a consumer can copy them
/// directly. This crate intentionally does not depend on the package format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomRecord {
    /// Stable room id within the detection result: `room_0`, `room_1`, ...
    pub id: String,
    /// Room type: `bedroom`, `kitchen`, `bathroom`, `office`, `living_dining`,
    /// or `empty`.
    #[serde(rename = "type")]
    pub room_type: String,
    /// Floor band index, 0-based.
    pub floor: u32,
    /// Floor area in square metres.
    pub area_m2: f32,
    /// Room volume in cubic metres: `area_m2` times the actual band height.
    ///
    /// The top band can be shorter than 3 m, so this is not always
    /// `area_m2 * 3.0`.
    pub volume_m3: f32,
    /// Number of beds in the room.
    #[serde(default)]
    pub beds: u32,
    /// Number of desks in the room, contributing job slots.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub desks: u32,
    /// Number of door material voxels adjacent to the room at the sampled
    /// layer.
    #[serde(default)]
    pub doors: u32,
    /// Number of window material voxels adjacent to the room at the sampled
    /// layer.
    #[serde(default)]
    pub windows: u32,
}

/// Bounding rectangle of one room on its floor band, in voxel cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomBounds {
    /// Left-most cell, inclusive.
    pub min_x: u32,
    /// Right-most cell, inclusive.
    pub max_x: u32,
    /// Top-most cell, inclusive.
    pub min_z: u32,
    /// Bottom-most cell, inclusive.
    pub max_z: u32,
    /// Floor band index.
    pub floor: u32,
    /// Height of this band in 10 cm voxels (the top band can be partial).
    pub band_height_voxels: u32,
}

/// Aggregate room and material stats for one volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomStats {
    /// Detected rooms in stable scan order.
    pub rooms: Vec<RoomRecord>,
    /// Non-air voxel counts keyed `material:{id}`.
    pub materials: BTreeMap<String, u64>,
    /// Total beds across all rooms.
    pub resident_capacity: u32,
    /// Total desks across all rooms.
    pub job_slots: u32,
    /// Simple building class: `lower_middle`, `middle`, or `upper`.
    pub class: String,
    /// Total non-air voxels.
    pub solid_voxels: u64,
    /// Per-room bounding rectangles, in the same order as `rooms`.
    ///
    /// Skipped by serde so the benchmark checksum remains focused on gameplay
    /// stats; shells use it for overlays.
    #[serde(default, skip_serializing)]
    pub room_bounds: Vec<RoomBounds>,
}

impl Default for RoomStats {
    fn default() -> Self {
        Self {
            rooms: Vec::new(),
            materials: BTreeMap::new(),
            resident_capacity: 0,
            job_slots: 0,
            class: "lower_middle".to_string(),
            solid_voxels: 0,
            room_bounds: Vec::new(),
        }
    }
}

/// Detect rooms and compute material/capacity stats for `volume`.
///
/// The volume is scanned in deterministic floor, then Z, then X order, so room
/// ids and the room list are stable for the same input.
#[must_use]
pub fn detect_rooms(
    volume: &VoxelVolume,
    materials: &RoomMaterials,
    props: &[PropInput],
) -> RoomStats {
    let (material_counts, solid_voxels) = count_materials(volume);
    let config = volume.config();
    let [width, height, depth] = config.voxels();
    let band_count = height.div_ceil(FLOOR_BAND_VOXELS as usize);

    let mut pending: Vec<PendingRoom> = Vec::new();
    let mut bands: Vec<BandLayout> = Vec::with_capacity(band_count);

    for floor in 0..band_count {
        let band_start = floor * FLOOR_BAND_VOXELS as usize;
        let band_end = (band_start + FLOOR_BAND_VOXELS as usize).min(height);
        // Sample the middle of the band. The final band can be partial, so a
        // fixed 1.5 m offset can land in a ceiling slab; the midpoint is the
        // most representative layer available and is 1.5 m for a full band.
        let sample_y = band_start + (band_end - band_start) / 2;

        let components = flood_band(volume, materials, width, depth, sample_y);
        let mut layout = BandLayout {
            width,
            depth,
            component: vec![NO_ROOM; width * depth],
        };

        for component in components {
            let room_index = pending.len();
            for &(x, z) in &component.cells {
                layout.component[z * width + x] = room_index;
            }
            let mut min_x = u32::MAX;
            let mut max_x = 0;
            let mut min_z = u32::MAX;
            let mut max_z = 0;
            for &(x, z) in &component.cells {
                min_x = min_x.min(x as u32);
                max_x = max_x.max(x as u32);
                min_z = min_z.min(z as u32);
                max_z = max_z.max(z as u32);
            }
            pending.push(PendingRoom {
                floor: floor as u32,
                cells: component.cells.len() as u32,
                doors: component.doors.len() as u32,
                windows: component.windows.len() as u32,
                beds: 0,
                desks: 0,
                stoves: 0,
                toilets: 0,
                showers: 0,
                sofas: 0,
                tables: 0,
                min_x,
                max_x,
                min_z,
                max_z,
                band_height_voxels: (band_end - band_start) as u32,
            });
        }

        bands.push(layout);
    }

    let volume_height = volume.config().voxels()[1];
    for prop in props {
        let Some((x, y, z)) = prop_voxel(prop) else {
            continue;
        };
        // The final band is partial. `y / FLOOR_BAND_VOXELS` can map a prop
        // above the volume's top voxel into that band, so reject it explicitly.
        if y >= volume_height {
            continue;
        }
        let Some(band) = bands.get(y / FLOOR_BAND_VOXELS as usize) else {
            continue;
        };
        if x >= band.width || z >= band.depth {
            continue;
        }
        let room_index = band.component[z * band.width + x];
        if room_index == NO_ROOM {
            continue;
        }

        let room = &mut pending[room_index];
        match prop.kind {
            PropKind::Bed => room.beds += 1,
            PropKind::Stove => room.stoves += 1,
            PropKind::Toilet => room.toilets += 1,
            PropKind::Shower => room.showers += 1,
            PropKind::Desk => room.desks += 1,
            PropKind::Sofa => room.sofas += 1,
            PropKind::Table => room.tables += 1,
            PropKind::Sink | PropKind::Other => {}
        }
    }

    let mut rooms = Vec::with_capacity(pending.len());
    let mut room_bounds = Vec::with_capacity(pending.len());
    let mut resident_capacity = 0;
    let mut job_slots = 0;
    for (index, room) in pending.into_iter().enumerate() {
        let area_m2 = room.cells as f32 * VOXEL_AREA_M2;
        resident_capacity += room.beds;
        job_slots += room.desks;
        rooms.push(RoomRecord {
            id: format!("room_{index}"),
            room_type: room.room_type().to_string(),
            floor: room.floor,
            area_m2,
            volume_m3: area_m2 * (room.band_height_voxels as f32 * VOXEL_SIZE_M),
            beds: room.beds,
            desks: room.desks,
            doors: room.doors,
            windows: room.windows,
        });
        room_bounds.push(RoomBounds {
            min_x: room.min_x,
            max_x: room.max_x,
            min_z: room.min_z,
            max_z: room.max_z,
            floor: room.floor,
            band_height_voxels: room.band_height_voxels,
        });
    }

    let class = building_class(&rooms, resident_capacity, job_slots).to_string();

    RoomStats {
        rooms,
        materials: material_counts,
        resident_capacity,
        job_slots,
        class,
        solid_voxels,
        room_bounds,
    }
}

const NO_ROOM: usize = usize::MAX;

struct PendingRoom {
    floor: u32,
    cells: u32,
    doors: u32,
    windows: u32,
    beds: u32,
    desks: u32,
    stoves: u32,
    toilets: u32,
    showers: u32,
    sofas: u32,
    tables: u32,
    min_x: u32,
    max_x: u32,
    min_z: u32,
    max_z: u32,
    band_height_voxels: u32,
}

impl PendingRoom {
    /// Priority: bedroom, kitchen, bathroom, office, living/dining, empty.
    fn room_type(&self) -> &'static str {
        if self.beds > 0 {
            "bedroom"
        } else if self.stoves > 0 {
            "kitchen"
        } else if self.toilets > 0 || self.showers > 0 {
            "bathroom"
        } else if self.desks > 0 {
            "office"
        } else if self.sofas > 0 || self.tables > 0 {
            "living_dining"
        } else {
            "empty"
        }
    }
}

struct BandLayout {
    width: usize,
    depth: usize,
    /// Room index per cell, or [`NO_ROOM`].
    component: Vec<usize>,
}

struct AirComponent {
    cells: Vec<(usize, usize)>,
    doors: BTreeSet<usize>,
    windows: BTreeSet<usize>,
}

fn flood_band(
    volume: &VoxelVolume,
    materials: &RoomMaterials,
    width: usize,
    depth: usize,
    y: usize,
) -> Vec<AirComponent> {
    let mut visited = vec![false; width * depth];
    let mut components = Vec::new();

    for z in 0..depth {
        for x in 0..width {
            let start = z * width + x;
            if visited[start] || material_at(volume, x, y, z) != AIR {
                continue;
            }

            visited[start] = true;
            let mut queue = VecDeque::new();
            queue.push_back(start);
            let mut cells = Vec::new();
            let mut doors = BTreeSet::new();
            let mut windows = BTreeSet::new();
            let mut touches_boundary = false;

            while let Some(index) = queue.pop_front() {
                let cell_x = index % width;
                let cell_z = index / width;
                cells.push((cell_x, cell_z));

                if cell_x == 0 || cell_z == 0 || cell_x + 1 == width || cell_z + 1 == depth {
                    touches_boundary = true;
                }

                for (neighbor_x, neighbor_z) in neighbors(cell_x, cell_z, width, depth) {
                    let neighbor = neighbor_z * width + neighbor_x;
                    match material_at(volume, neighbor_x, y, neighbor_z) {
                        AIR => {
                            if !visited[neighbor] {
                                visited[neighbor] = true;
                                queue.push_back(neighbor);
                            }
                        }
                        material if material == materials.door => {
                            doors.insert(neighbor);
                        }
                        material if material == materials.window => {
                            windows.insert(neighbor);
                        }
                        _ => {}
                    }
                }
            }

            if !touches_boundary {
                components.push(AirComponent {
                    cells,
                    doors,
                    windows,
                });
            }
        }
    }

    components
}

fn neighbors(
    x: usize,
    z: usize,
    width: usize,
    depth: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let mut result = [(0, 0); 4];
    let mut count = 0;
    if x > 0 {
        result[count] = (x - 1, z);
        count += 1;
    }
    if x + 1 < width {
        result[count] = (x + 1, z);
        count += 1;
    }
    if z > 0 {
        result[count] = (x, z - 1);
        count += 1;
    }
    if z + 1 < depth {
        result[count] = (x, z + 1);
        count += 1;
    }
    result.into_iter().take(count)
}

fn material_at(volume: &VoxelVolume, x: usize, y: usize, z: usize) -> MaterialId {
    volume
        .get_voxel(VolumePos::new(x as u32, y as u32, z as u32))
        .unwrap_or(AIR)
}

fn prop_voxel(prop: &PropInput) -> Option<(usize, usize, usize)> {
    let [x, y, z] = prop.position;
    if !x.is_finite() || !y.is_finite() || !z.is_finite() || x < 0.0 || y < 0.0 || z < 0.0 {
        return None;
    }
    Some((
        (x / VOXEL_SIZE_M).floor() as usize,
        (y / VOXEL_SIZE_M).floor() as usize,
        (z / VOXEL_SIZE_M).floor() as usize,
    ))
}

/// Count non-air voxels per material and in total.
///
/// Keys are `material:{id}`; the map is sorted by id. Shells use this to price
/// a volume without repeating room detection, which is the expensive part.
#[must_use]
pub fn count_materials(volume: &VoxelVolume) -> (BTreeMap<String, u64>, u64) {
    let config = volume.config();
    let mut by_id: BTreeMap<MaterialId, u64> = BTreeMap::new();
    let mut solid_voxels = 0u64;

    for chunk_z in 0..config.chunks_z {
        for chunk_y in 0..config.chunks_y {
            for chunk_x in 0..config.chunks_x {
                let Some(chunk) =
                    volume.chunk_at(core_voxel::ChunkPos::new(chunk_x, chunk_y, chunk_z))
                else {
                    continue;
                };
                for &material in chunk.as_slice() {
                    if material == AIR {
                        continue;
                    }
                    *by_id.entry(material).or_default() += 1;
                    solid_voxels += 1;
                }
            }
        }
    }

    debug_assert_eq!(solid_voxels, u64::from(volume.solid_count()));

    let materials = by_id
        .into_iter()
        .map(|(id, count)| (format!("material:{id}"), count))
        .collect();
    (materials, solid_voxels)
}

const fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn building_class(rooms: &[RoomRecord], resident_capacity: u32, job_slots: u32) -> &'static str {
    let area_m2: f32 = rooms.iter().map(|room| room.area_m2).sum();
    let amenities: u32 = rooms
        .iter()
        .map(|room| room.doors + room.windows)
        .sum::<u32>()
        + resident_capacity
        + job_slots;
    let score = area_m2 + amenities as f32 * 2.0;

    if score >= 60.0 {
        "upper"
    } else if score >= 20.0 {
        "middle"
    } else {
        "lower_middle"
    }
}
