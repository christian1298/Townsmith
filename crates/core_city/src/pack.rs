//! In-memory mod content pack v0.
//!
//! A [`ContentPack`] binds one validated [`ModManifest`] to the material, room,
//! and prop documents it contributes. Construction is deliberately explicit:
//! [`ContentPack::from_json`] takes the four document bodies as strings, so the
//! same code path will serve bundled fixtures, tests, an editor, or a future
//! package loader without any file I/O here.
//!
//! # Namespace rule
//!
//! A manifest ID is either a bare namespace (`mymod`) or a namespace plus pack
//! name (`mymod:pack`). The part before the first `:` owns the pack's content
//! IDs. A manifest with ID `example:copper_age` may define
//! `example:copper_sheet`, but not `base:copper_sheet` or `other:copper_sheet`.
//! The `base` namespace is reserved for the game; a pack cannot claim it.
//!
//! # Merge policy
//!
//! [`ContentPack::merged_with_base`] copies the base registry and then adds the
//! pack's definitions. There are no overrides and no load-order wins: a pack
//! may only add IDs. A shared ID is reported as
//! [`PackError::CollisionWithBase`]. The merged registry is validated before it
//! is returned, and its own checksum is deterministic.
//!
//! # What v0 does not do
//!
//! - It does not read the filesystem, discover packages, download mods, unpack
//!   ZIP archives, or execute scripts.
//! - It does not resolve dependency versions, load order, overrides, or save
//!   references. A pack is validated in isolation; `dependencies` are not
//!   resolved here.
//! - It does not resolve cross-mod references. Because the pack's registry is
//!   validated on its own, a prop's `allowed_rooms` entries must be rooms
//!   defined by the same pack; base rooms cannot be referenced in v0.
//! - It does not add Workshop support or make [[D-006]] accepted.
//!
//! # Example
//!
//! ```
//! use core_city::pack::ContentPack;
//!
//! let manifest = r#"{
//!     "format_version": 1,
//!     "id": "example:pack",
//!     "name": "Example Pack",
//!     "version": "1.0.0",
//!     "game_version": ">=0.1.0 <0.2.0",
//!     "content": {
//!         "materials": "materials.json",
//!         "rooms": "rooms.json",
//!         "props": "props.json"
//!     }
//! }"#;
//! let materials = r#"{
//!     "format_version": 1,
//!     "materials": [{
//!         "id": "example:plank",
//!         "name_key": "content.material.plank",
//!         "category": "wood",
//!         "density_kg_m3": 700,
//!         "color": [0.5, 0.4, 0.2],
//!         "cost_per_litre": 0.1,
//!         "tags": ["timber"]
//!     }]
//! }"#;
//! let rooms = r#"{
//!     "format_version": 1,
//!     "rooms": [{
//!         "id": "example:room",
//!         "name_key": "content.room.test",
//!         "category": "living",
//!         "min_area_m2": 9,
//!         "required_tags": ["bed"]
//!     }]
//! }"#;
//! let props = r#"{
//!     "format_version": 1,
//!     "props": [{
//!         "id": "example:prop",
//!         "name_key": "content.prop.test",
//!         "footprint_m": [1.6, 2.0],
//!         "height_m": 0.6,
//!         "cost": 10,
//!         "allowed_rooms": ["example:room"],
//!         "tags": ["sleep"]
//!     }]
//! }"#;
//!
//! let pack = ContentPack::from_json(manifest, materials, rooms, props).expect("valid pack");
//! assert_eq!(pack.namespace(), "example");
//! assert_eq!(pack.registry().material_count(), 1);
//! assert!(pack.merged_with_base().is_ok());
//! ```

use std::error::Error;
use std::fmt;

use crate::manifest::{ManifestError, ModManifest, MOD_FORMAT_VERSION};
use crate::registry::{ContentRegistry, Fnv1a, RegistryError};

/// A validated, in-memory mod content pack.
///
/// Build one with [`Self::from_json`]. The raw document strings are retained
/// for future packaging and save manifests; they are not re-parsed. Fields are
/// private so a pack cannot be mutated into an invalid state.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentPack {
    manifest: ModManifest,
    materials_json: String,
    rooms_json: String,
    props_json: String,
    registry: ContentRegistry,
    checksum: u64,
}

impl ContentPack {
    /// Parse and validate one manifest and its three content documents.
    ///
    /// Validation order is:
    ///
    /// 1. The manifest, using [`ModManifest::from_json_str`].
    /// 2. The pack's own registry, using [`ContentRegistry::from_json_strs`].
    ///    This also rejects duplicate IDs within the pack and prop references
    ///    to rooms that the pack does not define.
    /// 3. Every ID's namespace against [`ModManifest::namespace`].
    /// 4. Collisions with [`ContentRegistry::base`].
    /// 5. The reserved `base` namespace.
    ///
    /// # Errors
    ///
    /// Returns [`PackError::Manifest`] or [`PackError::Content`] for malformed
    /// documents, [`PackError::Namespace`] when an ID is outside the manifest's
    /// namespace, [`PackError::CollisionWithBase`] when an ID already exists in
    /// the base game, [`PackError::ReservedNamespace`] when the manifest claims
    /// `base`, and [`PackError::Base`] if the bundled base content cannot load.
    pub fn from_json(
        manifest_json: &str,
        materials_json: &str,
        rooms_json: &str,
        props_json: &str,
    ) -> Result<Self, PackError> {
        let manifest = ModManifest::from_json_str(manifest_json).map_err(PackError::Manifest)?;
        let registry = ContentRegistry::from_json_strs(materials_json, rooms_json, props_json)
            .map_err(PackError::Content)?;

        let namespace = manifest.namespace();
        for id in registry.ids() {
            if id_namespace(id) != Some(namespace) {
                return Err(PackError::Namespace {
                    namespace: namespace.to_owned(),
                    id: id.to_owned(),
                });
            }
        }

        let base = ContentRegistry::base().map_err(PackError::Base)?;
        for id in registry.ids() {
            if base.contains(id) {
                return Err(PackError::CollisionWithBase { id: id.to_owned() });
            }
        }
        if namespace == BASE_NAMESPACE {
            return Err(PackError::ReservedNamespace {
                namespace: namespace.to_owned(),
            });
        }

        let checksum = pack_checksum(&manifest, &registry);
        Ok(Self {
            manifest,
            materials_json: materials_json.to_owned(),
            rooms_json: rooms_json.to_owned(),
            props_json: props_json.to_owned(),
            registry,
            checksum,
        })
    }

    /// The validated manifest.
    #[must_use]
    pub fn manifest(&self) -> &ModManifest {
        &self.manifest
    }

    /// Content namespace owned by this pack, derived from the manifest ID.
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.manifest.namespace()
    }

    /// The pack's materials document body, as supplied to [`Self::from_json`].
    #[must_use]
    pub fn materials_json(&self) -> &str {
        &self.materials_json
    }

    /// The pack's rooms document body, as supplied to [`Self::from_json`].
    #[must_use]
    pub fn rooms_json(&self) -> &str {
        &self.rooms_json
    }

    /// The pack's props document body, as supplied to [`Self::from_json`].
    #[must_use]
    pub fn props_json(&self) -> &str {
        &self.props_json
    }

    /// The pack's validated registry, containing only the pack's definitions.
    #[must_use]
    pub fn registry(&self) -> &ContentRegistry {
        &self.registry
    }

    /// Stable 64-bit checksum of the pack.
    ///
    /// The checksum is FNV-1a 64 over the manifest checksum and the registry
    /// checksum. Both components are semantic: JSON key order, collection
    /// order, and tag array order do not change the result. It is a
    /// content-identity helper for manifests and regression comparisons, not a
    /// cryptographic hash.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        self.checksum
    }

    /// Merge the base game's content with this pack, rejecting shared IDs.
    ///
    /// The base registry is copied first and the pack's definitions are added.
    /// Base content never loses to a pack; there are no overrides or load-order
    /// wins. The merged registry is validated before it is returned, so its
    /// checksum is stable and its prop references resolve.
    ///
    /// # Errors
    ///
    /// Returns [`PackError::Base`] if the bundled base content cannot load,
    /// [`PackError::CollisionWithBase`] if the pack redefines a base ID, or
    /// [`PackError::Merge`] if the merged registry fails another invariant.
    pub fn merged_with_base(&self) -> Result<ContentRegistry, PackError> {
        let base = ContentRegistry::base().map_err(PackError::Base)?;
        match ContentRegistry::merge(&base, &self.registry) {
            Ok(merged) => Ok(merged),
            Err(RegistryError::DuplicateId { id }) => Err(PackError::CollisionWithBase { id }),
            Err(source) => Err(PackError::Merge(source)),
        }
    }
}

/// The namespace reserved for the base game.
const BASE_NAMESPACE: &str = "base";

fn id_namespace(id: &str) -> Option<&str> {
    id.split_once(':').map(|(namespace, _)| namespace)
}

fn pack_checksum(manifest: &ModManifest, registry: &ContentRegistry) -> u64 {
    let mut hash = Fnv1a::new();
    hash.write_str("townsmith:content-pack");
    hash.write_u32(MOD_FORMAT_VERSION);
    hash.write_u64(manifest.checksum());
    hash.write_u64(registry.checksum());
    hash.finish()
}

/// Errors returned while building or merging a [`ContentPack`].
#[derive(Debug)]
pub enum PackError {
    /// The manifest did not parse or validate.
    Manifest(ManifestError),
    /// The pack's content documents did not parse or validate.
    Content(RegistryError),
    /// The bundled base content could not be loaded.
    Base(RegistryError),
    /// A content ID is outside the namespace owned by the manifest.
    Namespace {
        /// Namespace owned by the manifest.
        namespace: String,
        /// Rejected content ID.
        id: String,
    },
    /// A content ID already exists in the base game.
    CollisionWithBase {
        /// Conflicting content ID.
        id: String,
    },
    /// The manifest claims the namespace reserved for the base game.
    ReservedNamespace {
        /// Rejected namespace.
        namespace: String,
    },
    /// The base and pack registries could not be merged.
    Merge(RegistryError),
}

impl fmt::Display for PackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(source) => write!(formatter, "invalid content pack manifest: {source}"),
            Self::Content(source) => write!(formatter, "invalid content pack content: {source}"),
            Self::Base(source) => write!(formatter, "base content failed to load: {source}"),
            Self::Namespace { namespace, id } => write!(
                formatter,
                "content id {id:?} is outside the pack namespace {namespace:?}"
            ),
            Self::CollisionWithBase { id } => {
                write!(formatter, "content id {id} collides with base content")
            }
            Self::ReservedNamespace { namespace } => write!(
                formatter,
                "content namespace {namespace:?} is reserved for the base game"
            ),
            Self::Merge(source) => write!(formatter, "content pack merge failed: {source}"),
        }
    }
}

impl Error for PackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Manifest(source) => Some(source),
            Self::Content(source) | Self::Base(source) | Self::Merge(source) => Some(source),
            Self::Namespace { .. }
            | Self::CollisionWithBase { .. }
            | Self::ReservedNamespace { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    const EXAMPLE_MANIFEST: &str = include_str!("../../../content/mods/example.mod.json");
    const EXAMPLE_MATERIALS: &str = include_str!("../../../content/mods/example/materials.json");
    const EXAMPLE_ROOMS: &str = include_str!("../../../content/mods/example/rooms.json");
    const EXAMPLE_PROPS: &str = include_str!("../../../content/mods/example/props.json");

    fn example_pack() -> ContentPack {
        ContentPack::from_json(
            EXAMPLE_MANIFEST,
            EXAMPLE_MATERIALS,
            EXAMPLE_ROOMS,
            EXAMPLE_PROPS,
        )
        .expect("example pack loads")
    }

    fn manifest_value() -> Value {
        json!({
            "format_version": 1,
            "id": "example:pack",
            "name": "Example Pack",
            "version": "1.0.0",
            "game_version": ">=0.1.0 <0.2.0",
            "content": {
                "materials": "materials.json",
                "rooms": "rooms.json",
                "props": "props.json"
            }
        })
    }

    fn manifest_json(value: &Value) -> String {
        serde_json::to_string(value).expect("manifest serializes")
    }

    fn documents(namespace: &str, material: &str) -> (String, String, String) {
        let materials = json!({
            "format_version": 1,
            "materials": [{
                "id": format!("{namespace}:{material}"),
                "name_key": "content.material.test",
                "category": "wood",
                "density_kg_m3": 700,
                "color": [0.5, 0.4, 0.2],
                "cost_per_litre": 0.1,
                "tags": ["test"]
            }]
        });
        let rooms = json!({
            "format_version": 1,
            "rooms": [{
                "id": format!("{namespace}:room"),
                "name_key": "content.room.test",
                "category": "living",
                "min_area_m2": 9,
                "required_tags": ["bed"]
            }]
        });
        let props = json!({
            "format_version": 1,
            "props": [{
                "id": format!("{namespace}:prop"),
                "name_key": "content.prop.test",
                "footprint_m": [1.6, 2.0],
                "height_m": 0.6,
                "cost": 10,
                "allowed_rooms": [format!("{namespace}:room")],
                "tags": ["sleep"]
            }]
        });
        (
            serde_json::to_string(&materials).expect("materials serialize"),
            serde_json::to_string(&rooms).expect("rooms serialize"),
            serde_json::to_string(&props).expect("props serialize"),
        )
    }

    fn build(manifest: &Value, namespace: &str, material: &str) -> Result<ContentPack, PackError> {
        let (materials, rooms, props) = documents(namespace, material);
        ContentPack::from_json(&manifest_json(manifest), &materials, &rooms, &props)
    }

    #[test]
    fn example_pack_loads_with_counts_and_stable_checksum() {
        let pack = example_pack();
        assert_eq!(pack.manifest().id, "example:copper_age");
        assert_eq!(pack.namespace(), "example");
        assert_eq!(pack.registry().material_count(), 3);
        assert_eq!(pack.registry().room_count(), 2);
        assert_eq!(pack.registry().prop_count(), 3);
        assert_eq!(pack.registry().len(), 8);
        assert!(pack.materials_json().contains("example:copper_sheet"));
        assert!(pack.rooms_json().contains("example:smelter_hall"));
        assert!(pack.props_json().contains("example:copper_smelter"));
        assert_eq!(pack.checksum(), 0xad4c_6458_69a4_11ed);
        assert_eq!(pack, example_pack());
    }

    #[test]
    fn merged_registry_adds_base_and_pack_counts() {
        let merged = example_pack().merged_with_base().expect("merge succeeds");
        assert_eq!(merged.material_count(), 11);
        assert_eq!(merged.room_count(), 9);
        assert_eq!(merged.prop_count(), 11);
        assert_eq!(merged.len(), 31);
        assert!(merged.material("base:oak_plank").is_some());
        assert!(merged.material("example:copper_sheet").is_some());
        assert!(merged.room("example:smelter_hall").is_some());
        assert!(merged.prop("example:copper_smelter").is_some());
        assert!(merged.validate().is_ok());
        assert_eq!(merged.checksum(), 0x330a_538d_0780_5b50);
        assert_eq!(
            merged.checksum(),
            example_pack()
                .merged_with_base()
                .expect("merge succeeds")
                .checksum()
        );
    }

    #[test]
    fn wrong_namespace_is_rejected() {
        let manifest = manifest_value();
        let error = build(&manifest, "other", "material").expect_err("wrong namespace");
        assert!(matches!(
            error,
            PackError::Namespace { namespace, id }
                if namespace == "example" && id == "other:material"
        ));

        let error = build(&manifest, "base", "oak_plank").expect_err("base namespace");
        assert!(matches!(
            error,
            PackError::Namespace { namespace, id }
                if namespace == "example" && id == "base:oak_plank"
        ));
    }

    #[test]
    fn duplicate_with_base_is_rejected() {
        let mut manifest = manifest_value();
        manifest["id"] = json!("base");
        let error = build(&manifest, "base", "oak_plank").expect_err("base collision");
        assert!(matches!(
            error,
            PackError::CollisionWithBase { id } if id == "base:oak_plank"
        ));
    }

    #[test]
    fn claiming_the_base_namespace_is_rejected() {
        let mut manifest = manifest_value();
        manifest["id"] = json!("base");
        let error = build(&manifest, "base", "unused_material").expect_err("reserved namespace");
        assert!(matches!(
            error,
            PackError::ReservedNamespace { namespace } if namespace == "base"
        ));
    }

    #[test]
    fn duplicate_ids_within_the_pack_are_rejected() {
        let manifest = manifest_value();
        let (materials, rooms, props) = documents("example", "material");
        let mut material_document: Value =
            serde_json::from_str(&materials).expect("materials parse");
        let repeat = material_document["materials"][0].clone();
        material_document["materials"]
            .as_array_mut()
            .expect("materials array")
            .push(repeat);
        let materials = serde_json::to_string(&material_document).expect("materials serialize");
        let error = ContentPack::from_json(&manifest_json(&manifest), &materials, &rooms, &props)
            .expect_err("duplicate material");
        assert!(matches!(
            error,
            PackError::Content(RegistryError::DuplicateId { id }) if id == "example:material"
        ));

        let (materials, rooms, props) = documents("example", "material");
        let mut rooms: Value = serde_json::from_str(&rooms).expect("rooms parse");
        rooms["rooms"][0]["id"] = json!("example:material");
        let rooms = serde_json::to_string(&rooms).expect("rooms serialize");
        let error = ContentPack::from_json(&manifest_json(&manifest), &materials, &rooms, &props)
            .expect_err("duplicate across collections");
        assert!(matches!(
            error,
            PackError::Content(RegistryError::DuplicateId { id }) if id == "example:material"
        ));
    }

    #[test]
    fn malformed_content_is_rejected() {
        let manifest = manifest_value();
        let (_materials, rooms, props) = documents("example", "material");

        let error = ContentPack::from_json(&manifest_json(&manifest), "{", &rooms, &props)
            .expect_err("malformed JSON");
        assert!(matches!(
            error,
            PackError::Content(RegistryError::Json { .. })
        ));

        let error = ContentPack::from_json(&manifest_json(&manifest), "{}", &rooms, &props)
            .expect_err("missing collections");
        assert!(matches!(
            error,
            PackError::Content(RegistryError::Json { .. })
        ));

        let mut materials: Value =
            serde_json::from_str(&documents("example", "material").0).expect("materials parse");
        materials["materials"][0]["density_kg_m3"] = json!(-1);
        let materials = serde_json::to_string(&materials).expect("materials serialize");
        let error = ContentPack::from_json(&manifest_json(&manifest), &materials, &rooms, &props)
            .expect_err("invalid density");
        assert!(matches!(
            error,
            PackError::Content(RegistryError::InvalidNumber {
                field: "density_kg_m3",
                ..
            })
        ));
    }

    #[test]
    fn manifest_mismatch_is_rejected() {
        let mut manifest = manifest_value();
        manifest["id"] = json!("Example:Bad");
        let (materials, rooms, props) = documents("example", "material");
        let error = ContentPack::from_json(&manifest_json(&manifest), &materials, &rooms, &props)
            .expect_err("invalid manifest");
        assert!(matches!(error, PackError::Manifest(_)));

        let mut manifest = manifest_value();
        manifest["format_version"] = json!(2);
        let error = ContentPack::from_json(&manifest_json(&manifest), &materials, &rooms, &props)
            .expect_err("unsupported manifest version");
        assert!(matches!(error, PackError::Manifest(_)));
    }

    #[test]
    fn checksum_is_independent_of_source_array_order() {
        let manifest = manifest_json(&manifest_value());
        let (materials, rooms, props) = documents("example", "material");
        let first =
            ContentPack::from_json(&manifest, &materials, &rooms, &props).expect("pack loads");

        let reordered = |document: &str, collection: &str| {
            let mut value: Value = serde_json::from_str(document).expect("document parses");
            value[collection]
                .as_array_mut()
                .expect("collection array")
                .reverse();
            serde_json::to_string(&value).expect("document serializes")
        };
        let second = ContentPack::from_json(
            &manifest,
            &reordered(&materials, "materials"),
            &reordered(&rooms, "rooms"),
            &reordered(&props, "props"),
        )
        .expect("pack loads");

        assert_eq!(second.checksum(), first.checksum());
        assert_eq!(second.registry().ids(), first.registry().ids());
    }

    #[test]
    fn checksum_changes_with_content_and_manifest() {
        let manifest = manifest_json(&manifest_value());
        let (materials, rooms, props) = documents("example", "material");
        let pack =
            ContentPack::from_json(&manifest, &materials, &rooms, &props).expect("pack loads");

        let mut changed: Value = serde_json::from_str(&materials).expect("materials parse");
        changed["materials"][0]["cost_per_litre"] = json!(0.2);
        let changed = serde_json::to_string(&changed).expect("materials serialize");
        let other =
            ContentPack::from_json(&manifest, &changed, &rooms, &props).expect("pack loads");
        assert_ne!(other.checksum(), pack.checksum());

        let mut manifest = manifest_value();
        manifest["version"] = json!("1.0.1");
        let other = build(&manifest, "example", "material").expect("pack loads");
        assert_ne!(other.checksum(), pack.checksum());
    }
}
