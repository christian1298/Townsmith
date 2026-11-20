//! `blueprint.json` metadata.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Current `.tsbp` metadata/package format.
pub const FORMAT_VERSION: u16 = 1;

/// One detected room in a blueprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoomRecord {
    /// Stable room id within the blueprint.
    pub id: String,
    /// Room type, such as `bedroom` or `kitchen`.
    #[serde(rename = "type")]
    pub room_type: String,
    /// Floor index, 0-based.
    pub floor: u32,
    /// Floor area in square metres.
    pub area_m2: f32,
    /// Room volume in cubic metres.
    pub volume_m3: f32,
    /// Bed count, when applicable.
    #[serde(default)]
    pub beds: u32,
    /// Door count.
    #[serde(default)]
    pub doors: u32,
    /// Window count.
    #[serde(default)]
    pub windows: u32,
}

/// One functional prop placed in a blueprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropRecord {
    /// Content id, for example `base:bed_single`.
    pub id: String,
    /// Position in metres.
    pub position: [f32; 3],
    /// Rotation in degrees around Y.
    #[serde(default)]
    pub rotation: f32,
}

/// Gameplay statistics derived from the blueprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlueprintStats {
    /// Resident capacity.
    #[serde(default)]
    pub resident_capacity: u32,
    /// Job slots.
    #[serde(default)]
    pub job_slots: u32,
    /// Class band, such as `lower_middle`.
    #[serde(default)]
    pub class: String,
}

impl Default for BlueprintStats {
    fn default() -> Self {
        Self {
            resident_capacity: 0,
            job_slots: 0,
            class: "lower_middle".to_string(),
        }
    }
}

/// Content dependencies required to load the blueprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependencies {
    /// Minimum game version expression.
    #[serde(default)]
    pub game: String,
    /// Required mod ids.
    #[serde(default)]
    pub mods: Vec<String>,
    /// Required resource pack ids.
    #[serde(default)]
    pub resource_packs: Vec<String>,
}

impl Default for Dependencies {
    fn default() -> Self {
        Self {
            game: ">=0.1.0".to_string(),
            mods: Vec::new(),
            resource_packs: Vec::new(),
        }
    }
}

/// Metadata stored in `blueprint.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlueprintMetadata {
    /// Package/metadata format version.
    pub format_version: u16,
    /// Stable content id, for example `local:river_house_01`.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Author display name.
    pub author: String,
    /// Search tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Volume bounds in metres `[x, y, z]`.
    pub bounds_m: [f32; 3],
    /// Number of non-air voxels.
    pub solid_voxels: u64,
    /// Material key to voxel/litre count.
    #[serde(default)]
    pub materials: BTreeMap<String, u64>,
    /// Detected rooms.
    #[serde(default)]
    pub rooms: Vec<RoomRecord>,
    /// Functional props.
    #[serde(default)]
    pub props: Vec<PropRecord>,
    /// Derived stats.
    pub stats: BlueprintStats,
    /// Content dependencies.
    pub dependencies: Dependencies,
    /// Canonical content hash, `blake3:<hex>`.
    #[serde(default)]
    pub content_hash: String,
}

impl BlueprintMetadata {
    /// Construct minimal metadata with the current format version.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            id: id.into(),
            name: name.into(),
            author: "player".to_string(),
            tags: Vec::new(),
            bounds_m: [0.0; 3],
            solid_voxels: 0,
            materials: BTreeMap::new(),
            rooms: Vec::new(),
            props: Vec::new(),
            stats: BlueprintStats::default(),
            dependencies: Dependencies::default(),
            content_hash: String::new(),
        }
    }
}
