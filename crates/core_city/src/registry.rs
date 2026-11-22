//! Runtime registry for v1 material, room, and prop content.
//!
//! The `content/base/*.json` files are the first data contracts for
//! [[D-006|data-first content]]. This module is the runtime half of that
//! spike: it parses the three collections, validates the shared ID space and
//! cross-references, and indexes every definition in a [`BTreeMap`] keyed by
//! its namespaced ID.
//!
//! [`ContentRegistry::base`] compiles the bundled files into the binary with
//! `include_str!`, so the registry works without a filesystem. There is no mod
//! manifest loader, package discovery, save-reference migration, or hot reload
//! yet; those remain open in D-006.
//!
//! Material, room, and prop IDs share one namespace. An ID such as
//! `base:oak_plank` must match `^[a-z0-9_]+:[a-z0-9_./-]+$` and may appear
//! only once across all three collections.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Content-file format version understood by this build.
pub const CONTENT_FORMAT_VERSION: u32 = 1;

const MATERIALS_DOCUMENT: &str = "materials.json";
const ROOMS_DOCUMENT: &str = "rooms.json";
const PROPS_DOCUMENT: &str = "props.json";

const BASE_MATERIALS_JSON: &str = include_str!("../../../content/base/materials.json");
const BASE_ROOMS_JSON: &str = include_str!("../../../content/base/rooms.json");
const BASE_PROPS_JSON: &str = include_str!("../../../content/base/props.json");

/// Broad material family from `material.schema.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialCategory {
    /// Wood and timber.
    Wood,
    /// Brick, stone, and other masonry.
    Masonry,
    /// Concrete and cementitious material.
    Concrete,
    /// Ferrous and non-ferrous metals.
    Metal,
    /// Glass and glazing.
    Glass,
    /// Earth, thatch, and other natural material.
    Earth,
    /// A material that does not fit the other families.
    Other,
}

impl MaterialCategory {
    /// Stable schema token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wood => "wood",
            Self::Masonry => "masonry",
            Self::Concrete => "concrete",
            Self::Metal => "metal",
            Self::Glass => "glass",
            Self::Earth => "earth",
            Self::Other => "other",
        }
    }
}

/// Broad room use from `room.schema.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomCategory {
    /// Sleeping, eating, and other household living space.
    Living,
    /// Kitchen, bathroom, and other service space.
    Service,
    /// Workshop, shop floor, office, and other work space.
    Work,
    /// Halls, stairs, and other circulation space.
    Circulation,
    /// Storage and utility space.
    Storage,
    /// A room that does not fit the other categories.
    Other,
}

impl RoomCategory {
    /// Stable schema token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Living => "living",
            Self::Service => "service",
            Self::Work => "work",
            Self::Circulation => "circulation",
            Self::Storage => "storage",
            Self::Other => "other",
        }
    }
}

/// One material definition from `content/base/materials.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialDef {
    /// Namespaced content ID, for example `base:oak_plank`.
    pub id: String,
    /// Localization key for the display name.
    pub name_key: String,
    /// Broad material family.
    pub category: MaterialCategory,
    /// Density in kilograms per cubic metre. Must be finite and positive.
    pub density_kg_m3: f64,
    /// Linear RGB color, each channel in `0.0..=1.0`.
    pub color: [f64; 3],
    /// Cost in abstract cost units per litre. Must be finite and non-negative.
    pub cost_per_litre: f64,
    /// Tags used for filtering, requirements, and mod queries.
    pub tags: Vec<String>,
    /// Draft marker. Draft content is not campaign-ready.
    #[serde(default)]
    pub draft: bool,
}

/// One room definition from `content/base/rooms.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDef {
    /// Namespaced content ID, for example `base:bedroom`.
    pub id: String,
    /// Localization key for the display name.
    pub name_key: String,
    /// Broad room use.
    pub category: RoomCategory,
    /// Minimum floor area in square metres. Must be finite and positive.
    pub min_area_m2: f64,
    /// Tags a room must provide to satisfy this room type.
    pub required_tags: Vec<String>,
    /// Draft marker. Draft content is not campaign-ready.
    #[serde(default)]
    pub draft: bool,
}

/// One prop definition from `content/base/props.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropDef {
    /// Namespaced content ID, for example `base:bed`.
    pub id: String,
    /// Localization key for the display name.
    pub name_key: String,
    /// Footprint as `[width, depth]` in metres. Both values must be positive.
    pub footprint_m: [f64; 2],
    /// Height in metres. Must be finite and positive.
    pub height_m: f64,
    /// Cost in abstract cost units. Must be finite and non-negative.
    pub cost: f64,
    /// Room IDs this prop may be placed in. Empty means unrestricted.
    pub allowed_rooms: Vec<String>,
    /// Tags used for filtering, requirements, and mod queries.
    pub tags: Vec<String>,
    /// Draft marker. Draft content is not campaign-ready.
    #[serde(default)]
    pub draft: bool,
}

/// Validated v1 content collections, indexed by namespaced ID.
///
/// IDs are unique across materials, rooms, and props. Definitions are stored
/// in [`BTreeMap`]s, so iteration and [`Self::ids`] are deterministic.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentRegistry {
    materials: BTreeMap<String, MaterialDef>,
    rooms: BTreeMap<String, RoomDef>,
    props: BTreeMap<String, PropDef>,
}

impl ContentRegistry {
    /// Content format version understood by this build.
    pub const FORMAT_VERSION: u32 = CONTENT_FORMAT_VERSION;

    /// Load the bundled `content/base/*.json` files.
    ///
    /// The files are compiled into the binary with `include_str!`; this never
    /// touches the filesystem and is the runtime path for the base game.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] if a bundled file is malformed, uses another
    /// format version, contains a duplicate or invalid ID, or fails a
    /// cross-reference check. The bundled fixture is covered by tests, so this
    /// normally succeeds.
    pub fn base() -> Result<Self, RegistryError> {
        Self::from_json_strs(BASE_MATERIALS_JSON, BASE_ROOMS_JSON, BASE_PROPS_JSON)
    }

    /// Parse and validate one JSON document per collection.
    ///
    /// `materials_json`, `rooms_json`, and `props_json` are the full document
    /// bodies, including each top-level `format_version` and collection array.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] when the JSON is malformed, a document uses an
    /// unsupported format version, a collection is empty, an ID is malformed or
    /// duplicated across collections, a name key is empty, a number is not
    /// finite or is outside its schema range, a tag is malformed or duplicated,
    /// or a prop references a room that does not exist.
    pub fn from_json_strs(
        materials_json: &str,
        rooms_json: &str,
        props_json: &str,
    ) -> Result<Self, RegistryError> {
        let materials: MaterialDocument = parse(MATERIALS_DOCUMENT, materials_json)?;
        let rooms: RoomDocument = parse(ROOMS_DOCUMENT, rooms_json)?;
        let props: PropDocument = parse(PROPS_DOCUMENT, props_json)?;

        check_format_version(MATERIALS_DOCUMENT, materials.format_version)?;
        check_format_version(ROOMS_DOCUMENT, rooms.format_version)?;
        check_format_version(PROPS_DOCUMENT, props.format_version)?;

        let registry = Self {
            materials: index_by_id(materials.materials, |definition| definition.id.as_str())?,
            rooms: index_by_id(rooms.rooms, |definition| definition.id.as_str())?,
            props: index_by_id(props.props, |definition| definition.id.as_str())?,
        };
        registry.validate()?;
        Ok(registry)
    }

    /// Registry format version.
    #[must_use]
    pub const fn format_version(&self) -> u32 {
        CONTENT_FORMAT_VERSION
    }

    /// Whether any collection defines `id`.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.materials.contains_key(id)
            || self.rooms.contains_key(id)
            || self.props.contains_key(id)
    }

    /// Material with `id`, if present.
    #[must_use]
    pub fn material(&self, id: &str) -> Option<&MaterialDef> {
        self.materials.get(id)
    }

    /// Room with `id`, if present.
    #[must_use]
    pub fn room(&self, id: &str) -> Option<&RoomDef> {
        self.rooms.get(id)
    }

    /// Prop with `id`, if present.
    #[must_use]
    pub fn prop(&self, id: &str) -> Option<&PropDef> {
        self.props.get(id)
    }

    /// Materials in ID order.
    pub fn materials(&self) -> impl Iterator<Item = &MaterialDef> {
        self.materials.values()
    }

    /// Rooms in ID order.
    pub fn rooms(&self) -> impl Iterator<Item = &RoomDef> {
        self.rooms.values()
    }

    /// Props in ID order.
    pub fn props(&self) -> impl Iterator<Item = &PropDef> {
        self.props.values()
    }

    /// Number of loaded materials.
    #[must_use]
    pub fn material_count(&self) -> usize {
        self.materials.len()
    }

    /// Number of loaded rooms.
    #[must_use]
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Number of loaded props.
    #[must_use]
    pub fn prop_count(&self) -> usize {
        self.props.len()
    }

    /// Total number of loaded definitions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.materials.len() + self.rooms.len() + self.props.len()
    }

    /// Whether the registry holds no definitions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every ID across materials, rooms, and props, sorted ascending.
    ///
    /// The returned order is stable regardless of the order in the source JSON.
    #[must_use]
    pub fn ids(&self) -> Vec<&str> {
        let mut ids = Vec::with_capacity(self.len());
        ids.extend(self.materials.keys().map(String::as_str));
        ids.extend(self.rooms.keys().map(String::as_str));
        ids.extend(self.props.keys().map(String::as_str));
        ids.sort_unstable();
        ids
    }

    /// Re-check every loaded definition and cross-reference.
    ///
    /// [`Self::from_json_strs`] calls this after indexing, so callers do not
    /// need to invoke it on a freshly loaded registry. It is public for tests,
    /// tooling, and callers that hold a registry across edits.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError`] if an invariant is broken. Repeated calls
    /// return the same result; the method does not mutate the registry.
    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.materials.is_empty() {
            return Err(RegistryError::EmptyCollection {
                document: MATERIALS_DOCUMENT,
            });
        }
        if self.rooms.is_empty() {
            return Err(RegistryError::EmptyCollection {
                document: ROOMS_DOCUMENT,
            });
        }
        if self.props.is_empty() {
            return Err(RegistryError::EmptyCollection {
                document: PROPS_DOCUMENT,
            });
        }

        let mut ids = BTreeSet::new();
        for definition in self.materials.values() {
            validate_material(definition, &mut ids)?;
        }
        for definition in self.rooms.values() {
            validate_room(definition, &mut ids)?;
        }
        for definition in self.props.values() {
            validate_prop(definition, &mut ids)?;
        }

        for prop in self.props.values() {
            for room in &prop.allowed_rooms {
                if !self.rooms.contains_key(room) {
                    return Err(RegistryError::MissingRoom {
                        prop: prop.id.clone(),
                        room: room.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Merge two validated registries into a new one, rejecting shared IDs.
    ///
    /// Definitions from `base` are copied first; `overlay` may only add IDs.
    /// There are no overrides: a shared ID in any collection returns
    /// [`RegistryError::DuplicateId`]. The merged registry is validated before
    /// it is returned.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::DuplicateId`] when `base` and `overlay` share an
    /// ID, or another [`RegistryError`] if the merged result fails validation,
    /// for example because an overlay prop references a room that is in neither
    /// registry.
    pub fn merge(base: &Self, overlay: &Self) -> Result<Self, RegistryError> {
        let mut merged = base.clone();
        for definition in overlay.materials.values() {
            if merged.contains(&definition.id) {
                return Err(RegistryError::DuplicateId {
                    id: definition.id.clone(),
                });
            }
            merged
                .materials
                .insert(definition.id.clone(), definition.clone());
        }
        for definition in overlay.rooms.values() {
            if merged.contains(&definition.id) {
                return Err(RegistryError::DuplicateId {
                    id: definition.id.clone(),
                });
            }
            merged
                .rooms
                .insert(definition.id.clone(), definition.clone());
        }
        for definition in overlay.props.values() {
            if merged.contains(&definition.id) {
                return Err(RegistryError::DuplicateId {
                    id: definition.id.clone(),
                });
            }
            merged
                .props
                .insert(definition.id.clone(), definition.clone());
        }
        merged.validate()?;
        Ok(merged)
    }

    /// Stable 64-bit checksum of the registry contents.
    ///
    /// The checksum is FNV-1a 64 over the format version and every definition
    /// in ID order. Tag, required-tag, and allowed-room arrays are sorted
    /// before hashing, so source array order does not change the value. It is
    /// intended for content manifests and regression comparisons, not as a
    /// cryptographic hash.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = Fnv1a::new();
        hash.write_str("townsmith:content");
        hash.write_u32(CONTENT_FORMAT_VERSION);

        hash.write_u64(self.materials.len() as u64);
        for definition in self.materials.values() {
            hash.write_str(&definition.id);
            hash.write_str(&definition.name_key);
            hash.write_str(definition.category.as_str());
            hash.write_f64(definition.density_kg_m3);
            for channel in definition.color {
                hash.write_f64(channel);
            }
            hash.write_f64(definition.cost_per_litre);
            write_sorted_strings(&mut hash, &definition.tags);
            hash.write_bool(definition.draft);
        }

        hash.write_u64(self.rooms.len() as u64);
        for definition in self.rooms.values() {
            hash.write_str(&definition.id);
            hash.write_str(&definition.name_key);
            hash.write_str(definition.category.as_str());
            hash.write_f64(definition.min_area_m2);
            write_sorted_strings(&mut hash, &definition.required_tags);
            hash.write_bool(definition.draft);
        }

        hash.write_u64(self.props.len() as u64);
        for definition in self.props.values() {
            hash.write_str(&definition.id);
            hash.write_str(&definition.name_key);
            for value in definition.footprint_m {
                hash.write_f64(value);
            }
            hash.write_f64(definition.height_m);
            hash.write_f64(definition.cost);
            write_sorted_strings(&mut hash, &definition.allowed_rooms);
            write_sorted_strings(&mut hash, &definition.tags);
            hash.write_bool(definition.draft);
        }

        hash.finish()
    }
}

/// Errors returned while parsing or validating content.
#[derive(Debug)]
pub enum RegistryError {
    /// A document did not parse as the expected v1 shape.
    Json {
        /// File label used in diagnostics.
        document: &'static str,
        /// Underlying parser error.
        source: serde_json::Error,
    },
    /// A document uses a format version this build cannot read.
    UnsupportedFormatVersion {
        /// File label used in diagnostics.
        document: &'static str,
        /// Version found in the document.
        found: u32,
    },
    /// A required collection is empty.
    EmptyCollection {
        /// File label used in diagnostics.
        document: &'static str,
    },
    /// Two definitions share an ID across one or more collections.
    DuplicateId {
        /// Conflicting ID.
        id: String,
    },
    /// An ID does not match the namespaced v1 pattern.
    InvalidId {
        /// Collection that owns the ID.
        collection: &'static str,
        /// Rejected ID.
        id: String,
    },
    /// A display-name localization key is empty.
    EmptyNameKey {
        /// Collection that owns the definition.
        collection: &'static str,
        /// Definition ID.
        id: String,
    },
    /// A number is non-finite, negative, zero where positive is required, or
    /// outside its schema range.
    InvalidNumber {
        /// Collection that owns the definition.
        collection: &'static str,
        /// Definition ID.
        id: String,
        /// JSON field name.
        field: &'static str,
        /// Rejected value.
        value: f64,
    },
    /// A tag does not match the v1 tag pattern.
    InvalidTag {
        /// Collection that owns the definition.
        collection: &'static str,
        /// Definition ID.
        id: String,
        /// Rejected tag.
        tag: String,
    },
    /// A tag appears more than once in one list.
    DuplicateTag {
        /// Collection that owns the definition.
        collection: &'static str,
        /// Definition ID.
        id: String,
        /// Repeated tag.
        tag: String,
    },
    /// A prop `allowed_rooms` entry is not a namespaced ID.
    InvalidReference {
        /// Prop ID.
        prop: String,
        /// Rejected room reference.
        room: String,
    },
    /// A prop lists the same room more than once.
    DuplicateReference {
        /// Prop ID.
        prop: String,
        /// Repeated room ID.
        room: String,
    },
    /// A prop allows a room that is not loaded.
    MissingRoom {
        /// Prop ID.
        prop: String,
        /// Referenced room ID.
        room: String,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { document, source } => {
                write!(formatter, "{document}: invalid content JSON: {source}")
            }
            Self::UnsupportedFormatVersion { document, found } => write!(
                formatter,
                "{document}: unsupported content format version {found}; expected {CONTENT_FORMAT_VERSION}"
            ),
            Self::EmptyCollection { document } => {
                write!(formatter, "{document}: collection must not be empty")
            }
            Self::DuplicateId { id } => {
                write!(formatter, "duplicate content id {id}")
            }
            Self::InvalidId { collection, id } => write!(
                formatter,
                "{collection} id {id:?} is not a namespaced content id"
            ),
            Self::EmptyNameKey { collection, id } => {
                write!(formatter, "{collection} {id} has an empty name_key")
            }
            Self::InvalidNumber {
                collection,
                id,
                field,
                value,
            } => write!(
                formatter,
                "{collection} {id} has invalid {field} value {value}"
            ),
            Self::InvalidTag {
                collection,
                id,
                tag,
            } => write!(formatter, "{collection} {id} has invalid tag {tag:?}"),
            Self::DuplicateTag {
                collection,
                id,
                tag,
            } => write!(formatter, "{collection} {id} repeats tag {tag:?}"),
            Self::InvalidReference { prop, room } => write!(
                formatter,
                "prop {prop} has invalid allowed_rooms entry {room:?}"
            ),
            Self::DuplicateReference { prop, room } => write!(
                formatter,
                "prop {prop} lists allowed room {room} more than once"
            ),
            Self::MissingRoom { prop, room } => {
                write!(formatter, "prop {prop} references unknown room {room}")
            }
        }
    }
}

impl Error for RegistryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialDocument {
    format_version: u32,
    materials: Vec<MaterialDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoomDocument {
    format_version: u32,
    rooms: Vec<RoomDef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PropDocument {
    format_version: u32,
    props: Vec<PropDef>,
}

fn parse<T: DeserializeOwned>(document: &'static str, json: &str) -> Result<T, RegistryError> {
    serde_json::from_str(json).map_err(|source| RegistryError::Json { document, source })
}

fn check_format_version(document: &'static str, found: u32) -> Result<(), RegistryError> {
    if found == CONTENT_FORMAT_VERSION {
        Ok(())
    } else {
        Err(RegistryError::UnsupportedFormatVersion { document, found })
    }
}

fn index_by_id<T>(
    definitions: Vec<T>,
    id: impl Fn(&T) -> &str,
) -> Result<BTreeMap<String, T>, RegistryError> {
    let mut indexed = BTreeMap::new();
    for definition in definitions {
        let key = id(&definition).to_owned();
        if indexed.insert(key.clone(), definition).is_some() {
            return Err(RegistryError::DuplicateId { id: key });
        }
    }
    Ok(indexed)
}

fn validate_material(
    definition: &MaterialDef,
    ids: &mut BTreeSet<String>,
) -> Result<(), RegistryError> {
    validate_id("material", &definition.id, ids)?;
    validate_name_key("material", &definition.id, &definition.name_key)?;
    validate_tags("material", &definition.id, &definition.tags)?;
    validate_positive(
        "material",
        &definition.id,
        "density_kg_m3",
        definition.density_kg_m3,
    )?;
    validate_non_negative(
        "material",
        &definition.id,
        "cost_per_litre",
        definition.cost_per_litre,
    )?;
    for channel in definition.color {
        if !channel.is_finite() || !(0.0..=1.0).contains(&channel) {
            return Err(RegistryError::InvalidNumber {
                collection: "material",
                id: definition.id.clone(),
                field: "color",
                value: channel,
            });
        }
    }
    Ok(())
}

fn validate_room(definition: &RoomDef, ids: &mut BTreeSet<String>) -> Result<(), RegistryError> {
    validate_id("room", &definition.id, ids)?;
    validate_name_key("room", &definition.id, &definition.name_key)?;
    validate_tags("room", &definition.id, &definition.required_tags)?;
    validate_positive(
        "room",
        &definition.id,
        "min_area_m2",
        definition.min_area_m2,
    )
}

fn validate_prop(definition: &PropDef, ids: &mut BTreeSet<String>) -> Result<(), RegistryError> {
    validate_id("prop", &definition.id, ids)?;
    validate_name_key("prop", &definition.id, &definition.name_key)?;
    validate_tags("prop", &definition.id, &definition.tags)?;
    for value in definition.footprint_m {
        validate_positive("prop", &definition.id, "footprint_m", value)?;
    }
    validate_positive("prop", &definition.id, "height_m", definition.height_m)?;
    validate_non_negative("prop", &definition.id, "cost", definition.cost)?;

    let mut rooms = BTreeSet::new();
    for room in &definition.allowed_rooms {
        if !is_valid_id(room) {
            return Err(RegistryError::InvalidReference {
                prop: definition.id.clone(),
                room: room.clone(),
            });
        }
        if !rooms.insert(room.as_str()) {
            return Err(RegistryError::DuplicateReference {
                prop: definition.id.clone(),
                room: room.clone(),
            });
        }
    }
    Ok(())
}

fn validate_id(
    collection: &'static str,
    id: &str,
    ids: &mut BTreeSet<String>,
) -> Result<(), RegistryError> {
    if !is_valid_id(id) {
        return Err(RegistryError::InvalidId {
            collection,
            id: id.to_owned(),
        });
    }
    if !ids.insert(id.to_owned()) {
        return Err(RegistryError::DuplicateId { id: id.to_owned() });
    }
    Ok(())
}

fn validate_name_key(
    collection: &'static str,
    id: &str,
    name_key: &str,
) -> Result<(), RegistryError> {
    if name_key.trim().is_empty() {
        return Err(RegistryError::EmptyNameKey {
            collection,
            id: id.to_owned(),
        });
    }
    Ok(())
}

fn validate_tags(collection: &'static str, id: &str, tags: &[String]) -> Result<(), RegistryError> {
    let mut seen = BTreeSet::new();
    for tag in tags {
        if !is_valid_tag(tag) {
            return Err(RegistryError::InvalidTag {
                collection,
                id: id.to_owned(),
                tag: tag.clone(),
            });
        }
        if !seen.insert(tag.as_str()) {
            return Err(RegistryError::DuplicateTag {
                collection,
                id: id.to_owned(),
                tag: tag.clone(),
            });
        }
    }
    Ok(())
}

fn validate_positive(
    collection: &'static str,
    id: &str,
    field: &'static str,
    value: f64,
) -> Result<(), RegistryError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(RegistryError::InvalidNumber {
            collection,
            id: id.to_owned(),
            field,
            value,
        })
    }
}

fn validate_non_negative(
    collection: &'static str,
    id: &str,
    field: &'static str,
    value: f64,
) -> Result<(), RegistryError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(RegistryError::InvalidNumber {
            collection,
            id: id.to_owned(),
            field,
            value,
        })
    }
}

/// Whether `id` matches `^[a-z0-9_]+:[a-z0-9_./-]+$`.
fn is_valid_id(id: &str) -> bool {
    let Some((namespace, path)) = id.split_once(':') else {
        return false;
    };
    if namespace.is_empty() || path.is_empty() || path.contains(':') {
        return false;
    }
    namespace
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && path.bytes().all(is_path_byte)
}

/// Whether `tag` matches `^[a-z0-9_./-]+$`.
fn is_valid_tag(tag: &str) -> bool {
    !tag.is_empty() && tag.bytes().all(is_path_byte)
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'/' | b'-')
}

fn write_sorted_strings(hash: &mut Fnv1a, values: &[String]) {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    hash.write_u64(sorted.len() as u64);
    for value in sorted {
        hash.write_str(value);
    }
}

/// FNV-1a 64-bit hasher shared by content and mod-manifest checksums.
pub(crate) struct Fnv1a(u64);

impl Fnv1a {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    pub(crate) fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn write_u8(&mut self, value: u8) {
        self.0 ^= u64::from(value);
        self.0 = self.0.wrapping_mul(Self::PRIME);
    }

    pub(crate) fn write_u32(&mut self, value: u32) {
        for byte in value.to_le_bytes() {
            self.write_u8(byte);
        }
    }

    pub(crate) fn write_u64(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.write_u8(byte);
        }
    }

    fn write_f64(&mut self, value: f64) {
        let bits = if value == 0.0 {
            0.0_f64.to_bits()
        } else {
            value.to_bits()
        };
        self.write_u64(bits);
    }

    pub(crate) fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub(crate) fn write_str(&mut self, value: &str) {
        self.write_u64(value.len() as u64);
        for byte in value.bytes() {
            self.write_u8(byte);
        }
    }

    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    fn base_documents() -> (Value, Value, Value) {
        (
            serde_json::from_str(BASE_MATERIALS_JSON).expect("base materials parse"),
            serde_json::from_str(BASE_ROOMS_JSON).expect("base rooms parse"),
            serde_json::from_str(BASE_PROPS_JSON).expect("base props parse"),
        )
    }

    fn load(
        materials: &Value,
        rooms: &Value,
        props: &Value,
    ) -> Result<ContentRegistry, RegistryError> {
        let materials = serde_json::to_string(materials).expect("materials serialize");
        let rooms = serde_json::to_string(rooms).expect("rooms serialize");
        let props = serde_json::to_string(props).expect("props serialize");
        ContentRegistry::from_json_strs(&materials, &rooms, &props)
    }

    #[test]
    fn base_registry_loads_counts_and_stable_checksum() {
        let registry = ContentRegistry::base().expect("base content loads");
        assert_eq!(registry.format_version(), CONTENT_FORMAT_VERSION);
        assert_eq!(registry.material_count(), 8);
        assert_eq!(registry.room_count(), 7);
        assert_eq!(registry.prop_count(), 8);
        assert_eq!(registry.len(), 23);
        assert_eq!(registry.checksum(), 0xc2df_0f24_2f5e_78d2);
        assert!(registry.validate().is_ok());
    }

    #[test]
    fn base_lookups_resolve_and_miss() {
        let registry = ContentRegistry::base().expect("base content loads");

        let oak = registry.material("base:oak_plank").expect("oak plank");
        assert_eq!(oak.category, MaterialCategory::Wood);
        assert_eq!(oak.density_kg_m3, 700.0);
        assert_eq!(oak.color, [0.55, 0.38, 0.22]);
        assert!(oak.draft);

        let bedroom = registry.room("base:bedroom").expect("bedroom");
        assert_eq!(bedroom.category, RoomCategory::Living);
        assert_eq!(bedroom.min_area_m2, 9.0);
        assert_eq!(bedroom.required_tags, ["bed"]);

        let bed = registry.prop("base:bed").expect("bed");
        assert_eq!(bed.footprint_m, [1.6, 2.0]);
        assert_eq!(bed.allowed_rooms, ["base:bedroom"]);

        assert!(registry.material("base:missing").is_none());
        assert!(registry.room("base:missing").is_none());
        assert!(registry.prop("base:missing").is_none());
    }

    #[test]
    fn contains_checks_all_collections() {
        let registry = ContentRegistry::base().expect("base content loads");
        assert!(registry.contains("base:oak_plank"));
        assert!(registry.contains("base:bedroom"));
        assert!(registry.contains("base:bed"));
        assert!(!registry.contains("base:missing"));
        assert!(!registry.contains("other:oak_plank"));
    }

    #[test]
    fn merge_adds_disjoint_registries_and_rejects_collisions() {
        let base = ContentRegistry::base().expect("base content loads");
        let overlay = test_registry("test");
        let merged = ContentRegistry::merge(&base, &overlay).expect("disjoint merge");
        assert_eq!(merged.len(), base.len() + overlay.len());
        assert_eq!(merged.material_count(), base.material_count() + 1);
        assert_eq!(merged.room_count(), base.room_count() + 1);
        assert_eq!(merged.prop_count(), base.prop_count() + 1);
        assert!(merged.contains("base:oak_plank"));
        assert!(merged.contains("test:material"));
        assert!(merged.validate().is_ok());
        assert_eq!(base.len(), 23, "merge must not mutate its inputs");
        assert_eq!(overlay.len(), 3, "merge must not mutate its inputs");

        let error = ContentRegistry::merge(&base, &base).expect_err("shared ids");
        assert!(matches!(
            error,
            RegistryError::DuplicateId { id } if id == "base:brick"
        ));
    }

    fn test_registry(namespace: &str) -> ContentRegistry {
        let materials = format!(
            r#"{{"format_version":1,"materials":[{{"id":"{namespace}:material","name_key":"content.material.test","category":"wood","density_kg_m3":700,"color":[0.5,0.4,0.2],"cost_per_litre":0.1,"tags":["test"]}}]}}"#
        );
        let rooms = format!(
            r#"{{"format_version":1,"rooms":[{{"id":"{namespace}:room","name_key":"content.room.test","category":"living","min_area_m2":9,"required_tags":["bed"]}}]}}"#
        );
        let props = format!(
            r#"{{"format_version":1,"props":[{{"id":"{namespace}:prop","name_key":"content.prop.test","footprint_m":[1.6,2.0],"height_m":0.6,"cost":10,"allowed_rooms":["{namespace}:room"],"tags":["sleep"]}}]}}"#
        );
        ContentRegistry::from_json_strs(&materials, &rooms, &props).expect("test registry loads")
    }

    #[test]
    fn ids_are_globally_sorted_and_unique() {
        let registry = ContentRegistry::base().expect("base content loads");
        let ids = registry.ids();
        assert_eq!(ids.len(), 23);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids.contains(&"base:oak_plank"));
        assert!(ids.contains(&"base:bedroom"));
        assert!(ids.contains(&"base:bed"));
    }

    #[test]
    fn duplicate_ids_are_rejected_within_and_across_collections() {
        let (mut materials, rooms, props) = base_documents();

        let duplicate = materials["materials"][0].clone();
        materials["materials"]
            .as_array_mut()
            .expect("materials array")
            .push(duplicate);
        let error = load(&materials, &rooms, &props).expect_err("duplicate material");
        assert!(matches!(error, RegistryError::DuplicateId { .. }));

        let (materials, mut rooms, props) = base_documents();
        let duplicate = materials["materials"][0]["id"].clone();
        rooms["rooms"][0]["id"] = duplicate;
        let error = load(&materials, &rooms, &props).expect_err("duplicate across collections");
        assert!(matches!(error, RegistryError::DuplicateId { .. }));
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for bad_id in [
            "oak_plank",
            ":oak_plank",
            "base:",
            "Base:oak_plank",
            "base:Oak_plank",
            "base:oak plank",
            "base:oak:extra",
        ] {
            let (mut materials, rooms, props) = base_documents();
            materials["materials"][0]["id"] = json!(bad_id);
            let error = load(&materials, &rooms, &props).expect_err(bad_id);
            assert!(
                matches!(error, RegistryError::InvalidId { .. }),
                "{bad_id}: {error}"
            );
        }
    }

    #[test]
    fn empty_name_keys_are_rejected() {
        for empty in ["", "   "] {
            let (mut materials, rooms, props) = base_documents();
            materials["materials"][0]["name_key"] = json!(empty);
            let error = load(&materials, &rooms, &props).expect_err("empty name key");
            assert!(
                matches!(error, RegistryError::EmptyNameKey { .. }),
                "{error}"
            );
        }
    }

    #[test]
    fn missing_room_references_are_rejected() {
        let (materials, rooms, mut props) = base_documents();
        props["props"][0]["allowed_rooms"] = json!(["base:missing"]);
        let error = load(&materials, &rooms, &props).expect_err("unknown room");
        match error {
            RegistryError::MissingRoom { prop, room } => {
                assert_eq!(prop, "base:bed");
                assert_eq!(room, "base:missing");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn unsupported_format_versions_are_rejected() {
        let (mut materials, rooms, props) = base_documents();
        materials["format_version"] = json!(2);
        let error = load(&materials, &rooms, &props).expect_err("bad version");
        assert!(matches!(
            error,
            RegistryError::UnsupportedFormatVersion { found: 2, .. }
        ));
    }

    #[test]
    fn negative_and_out_of_range_numbers_are_rejected() {
        let (mut materials, rooms, props) = base_documents();
        materials["materials"][0]["density_kg_m3"] = json!(-1.0);
        let error = load(&materials, &rooms, &props).expect_err("negative density");
        assert!(matches!(
            error,
            RegistryError::InvalidNumber {
                field: "density_kg_m3",
                ..
            }
        ));

        let (mut materials, rooms, props) = base_documents();
        materials["materials"][0]["color"][1] = json!(1.5);
        let error = load(&materials, &rooms, &props).expect_err("color out of range");
        assert!(matches!(
            error,
            RegistryError::InvalidNumber { field: "color", .. }
        ));

        let (materials, rooms, mut props) = base_documents();
        props["props"][0]["height_m"] = json!(0.0);
        let error = load(&materials, &rooms, &props).expect_err("zero height");
        assert!(matches!(
            error,
            RegistryError::InvalidNumber {
                field: "height_m",
                ..
            }
        ));
    }

    #[test]
    fn non_finite_numbers_are_rejected_by_validate() {
        let mut registry = ContentRegistry::base().expect("base content loads");
        registry
            .materials
            .get_mut("base:oak_plank")
            .expect("oak")
            .density_kg_m3 = f64::INFINITY;
        assert!(matches!(
            registry.validate(),
            Err(RegistryError::InvalidNumber {
                field: "density_kg_m3",
                ..
            })
        ));

        registry
            .materials
            .get_mut("base:oak_plank")
            .expect("oak")
            .density_kg_m3 = 700.0;
        registry.props.get_mut("base:bed").expect("bed").cost = f64::NAN;
        assert!(matches!(
            registry.validate(),
            Err(RegistryError::InvalidNumber { field: "cost", .. })
        ));
    }

    #[test]
    fn empty_collections_are_rejected() {
        let (materials, rooms, props) = base_documents();
        let mut empty = materials.clone();
        empty["materials"] = json!([]);
        let error = load(&empty, &rooms, &props).expect_err("empty materials");
        assert!(matches!(
            error,
            RegistryError::EmptyCollection {
                document: MATERIALS_DOCUMENT
            }
        ));
    }

    #[test]
    fn checksum_ignores_source_array_order() {
        let (mut materials, mut rooms, mut props) = base_documents();
        let registry = load(&materials, &rooms, &props).expect("base content loads");

        materials["materials"]
            .as_array_mut()
            .expect("materials array")
            .reverse();
        rooms["rooms"]
            .as_array_mut()
            .expect("rooms array")
            .reverse();
        props["props"]
            .as_array_mut()
            .expect("props array")
            .reverse();
        let reversed = load(&materials, &rooms, &props).expect("reordered base content loads");

        assert_eq!(reversed.ids(), registry.ids());
        assert_eq!(reversed.checksum(), registry.checksum());
    }

    #[test]
    fn definitions_round_trip_through_serde() {
        let (materials, rooms, props) = base_documents();

        for value in materials["materials"].as_array().expect("materials") {
            let definition: MaterialDef =
                serde_json::from_value(value.clone()).expect("material deserializes");
            let encoded = serde_json::to_string(&definition).expect("material serializes");
            let decoded: MaterialDef =
                serde_json::from_str(&encoded).expect("material deserializes again");
            assert_eq!(decoded, definition);
        }
        for value in rooms["rooms"].as_array().expect("rooms") {
            let definition: RoomDef =
                serde_json::from_value(value.clone()).expect("room deserializes");
            let encoded = serde_json::to_string(&definition).expect("room serializes");
            let decoded: RoomDef = serde_json::from_str(&encoded).expect("room deserializes again");
            assert_eq!(decoded, definition);
        }
        for value in props["props"].as_array().expect("props") {
            let definition: PropDef =
                serde_json::from_value(value.clone()).expect("prop deserializes");
            let encoded = serde_json::to_string(&definition).expect("prop serializes");
            let decoded: PropDef = serde_json::from_str(&encoded).expect("prop deserializes again");
            assert_eq!(decoded, definition);
        }
    }

    #[test]
    fn validate_is_idempotent() {
        let registry = ContentRegistry::base().expect("base content loads");
        registry.validate().expect("first validation");
        registry.validate().expect("second validation");
        assert_eq!(
            registry.checksum(),
            ContentRegistry::base()
                .expect("base content loads")
                .checksum()
        );
    }
}
