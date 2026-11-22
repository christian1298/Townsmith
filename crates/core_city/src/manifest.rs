//! Validated mod manifest v0 for the data-first content contract.
//!
//! [[D-006|Data-first content]] keeps content in JSON with namespaced IDs. The
//! base game compiles `content/base/*.json` into [`ContentRegistry`]; a mod
//! will eventually point at the same document shapes from its own directory.
//! This module is the first, deliberately small step: it parses and validates
//! a `mod.json`-style manifest, checks a set of manifests for duplicate IDs,
//! missing dependencies, and dependency cycles, and rejects duplicate content
//! IDs across already-loaded registries.
//!
//! # What v0 does not do
//!
//! - It does not read the filesystem, discover mods, download packages, unpack
//!   archives, or execute scripts.
//! - It does not load the content documents named by [`ModContentPaths`], so a
//!   manifest can validate before its files are installed.
//! - It does not resolve version ranges, load order, overrides, or save
//!   migrations. `game_version` is retained as an opaque, non-empty string.
//! - It does not make D-006 accepted. Scripting, packaging, Workshop support,
//!   and hot reload remain open.
//!
//! # IDs and namespaces
//!
//! A mod ID is either a bare namespace (`mymod`) or a namespace plus pack name
//! (`mymod:pack`), matching `^[a-z0-9_]+(:[a-z0-9_]+)?$`. The namespace before
//! the first `:` owns the mod's content IDs: a manifest with ID `mymod:pack`
//! may define `mymod:copper_roof`, never `base:copper_roof`. The base game owns
//! `base`. Dependency entries use the exact same ID form as top-level IDs.
//!
//! # Example
//!
//! ```
//! use core_city::manifest::ModManifest;
//!
//! let manifest = ModManifest::from_json_str(
//!     r#"{
//!         "format_version": 1,
//!         "id": "example:pack",
//!         "name": "Example Pack",
//!         "version": "1.0.0",
//!         "game_version": ">=0.1.0 <0.2.0",
//!         "content": {
//!             "materials": "content/materials.json",
//!             "rooms": "content/rooms.json",
//!             "props": "content/props.json"
//!         }
//!     }"#,
//! )
//! .expect("valid manifest");
//!
//! assert_eq!(manifest.id, "example:pack");
//! assert!(core_city::manifest::validate_set(&[manifest]).is_ok());
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::registry::{ContentRegistry, Fnv1a};

/// Mod-manifest format version understood by this build.
pub const MOD_FORMAT_VERSION: u32 = 1;

/// Relative paths to the content documents a mod contributes.
///
/// The named files use the same document shapes as `content/base/*.json`
/// (`format_version: 1` plus a non-empty collection array). v0 validates the
/// paths but does not open them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModContentPaths {
    /// Relative path to the mod's materials document.
    pub materials: String,
    /// Relative path to the mod's rooms document.
    pub rooms: String,
    /// Relative path to the mod's props document.
    pub props: String,
}

impl ModContentPaths {
    fn entries(&self) -> [(&'static str, &str); 3] {
        [
            ("materials", &self.materials),
            ("rooms", &self.rooms),
            ("props", &self.props),
        ]
    }
}

/// One validated mod manifest.
///
/// Build one with [`ModManifest::from_json_str`], which parses and validates.
/// The fields are public so callers can inspect and compare manifests; call
/// [`ModManifest::validate`] again after editing one, and
/// [`validate_set`] before accepting a collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModManifest {
    /// Manifest format version. Must be [`MOD_FORMAT_VERSION`].
    pub format_version: u32,
    /// Mod ID, either `mymod` or `mymod:pack`.
    pub id: String,
    /// Human-readable name. Must not be empty or whitespace only.
    pub name: String,
    /// Mod version, for example `1.0.0` or `1.0.0-beta.1`.
    pub version: String,
    /// Compatible game version or range. v0 stores this verbatim.
    pub game_version: String,
    /// Optional human-readable summary. Empty descriptions are rejected.
    #[serde(default)]
    pub description: Option<String>,
    /// Relative paths to this mod's content documents.
    pub content: ModContentPaths,
    /// Exact IDs of required mods. Duplicates and invalid IDs are rejected.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Draft marker for authoring. Draft manifests are not release-ready.
    #[serde(default)]
    pub draft: bool,
}

impl ModManifest {
    /// Manifest format version understood by this build.
    pub const FORMAT_VERSION: u32 = MOD_FORMAT_VERSION;

    /// Content namespace this manifest owns.
    ///
    /// A manifest ID is either a bare namespace (`mymod`) or a namespace plus
    /// pack name (`mymod:pack`). In both forms the part before the first `:`
    /// is the content namespace, so `mymod:pack` may define `mymod:*` IDs but
    /// never `base:*` or `other:*` IDs.
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.id
            .split_once(':')
            .map_or(self.id.as_str(), |(namespace, _)| namespace)
    }

    /// Parse and validate one manifest document.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] when the JSON is malformed, contains an
    /// unknown field, uses another format version, has an invalid mod ID,
    /// version, or content path, or repeats a dependency. Dependency existence
    /// and cycles are set-level checks; call [`validate_set`] for those.
    pub fn from_json_str(json: &str) -> Result<Self, ManifestError> {
        let manifest: Self =
            serde_json::from_str(json).map_err(|source| ManifestError::Json { source })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Re-check the manifest's local invariants.
    ///
    /// [`Self::from_json_str`] calls this after parsing, so callers do not need
    /// to invoke it on a freshly parsed manifest. It is public for tests,
    /// tooling, and callers that edit a manifest's public fields.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] if the format version, ID, name, version,
    /// description, content paths, or dependency list is invalid. The method
    /// does not mutate the manifest, so repeated calls return the same result.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.format_version != MOD_FORMAT_VERSION {
            return Err(ManifestError::UnsupportedFormatVersion {
                found: self.format_version,
            });
        }
        if !is_valid_mod_id(&self.id) {
            return Err(ManifestError::InvalidId {
                id: self.id.clone(),
            });
        }
        if self.name.trim().is_empty() {
            return Err(ManifestError::EmptyField { field: "name" });
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::EmptyField { field: "version" });
        }
        if !is_semver_like(&self.version) {
            return Err(ManifestError::InvalidVersion {
                version: self.version.clone(),
            });
        }
        if self.game_version.trim().is_empty() {
            return Err(ManifestError::EmptyField {
                field: "game_version",
            });
        }
        if self
            .description
            .as_deref()
            .is_some_and(|description| description.trim().is_empty())
        {
            return Err(ManifestError::EmptyField {
                field: "description",
            });
        }
        for (field, path) in self.content.entries() {
            if let Err(reason) = check_content_path(path) {
                return Err(ManifestError::InvalidPath {
                    field,
                    path: path.to_owned(),
                    reason,
                });
            }
        }

        let mut seen = BTreeSet::new();
        for dependency in &self.dependencies {
            if !is_valid_mod_id(dependency) {
                return Err(ManifestError::InvalidDependency {
                    id: self.id.clone(),
                    dependency: dependency.clone(),
                });
            }
            if !seen.insert(dependency.as_str()) {
                return Err(ManifestError::DuplicateDependency {
                    id: self.id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
        Ok(())
    }

    /// Stable 64-bit checksum of this manifest.
    ///
    /// The checksum is FNV-1a 64 over the format version and every manifest
    /// field. Dependency order is normalized, so reordering the JSON array does
    /// not change the value. It is a content-identity helper for manifests and
    /// regression comparisons, not a cryptographic hash, and it does not imply
    /// that the manifest is valid.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        let mut hash = Fnv1a::new();
        hash.write_str("townsmith:mod");
        hash.write_u32(self.format_version);
        hash.write_str(&self.id);
        hash.write_str(&self.name);
        hash.write_str(&self.version);
        hash.write_str(&self.game_version);
        match &self.description {
            Some(description) => {
                hash.write_bool(true);
                hash.write_str(description);
            }
            None => hash.write_bool(false),
        }
        for (_, path) in self.content.entries() {
            hash.write_str(path);
        }

        let mut dependencies: Vec<&str> = self.dependencies.iter().map(String::as_str).collect();
        dependencies.sort_unstable();
        hash.write_u64(dependencies.len() as u64);
        for dependency in dependencies {
            hash.write_str(dependency);
        }
        hash.write_bool(self.draft);
        hash.finish()
    }
}

/// Validate a collection of manifests as one dependency graph.
///
/// An empty set is valid. Every manifest is re-validated first; then the set
/// rejects duplicate IDs, dependencies that are not present, and dependency
/// cycles. Dependency IDs must match exactly; v0 does not parse version
/// ranges. The error for a cycle lists the cycle starting at the repeated ID,
/// for example `["a:b", "a:c", "a:b"]`.
///
/// # Errors
///
/// Returns [`ManifestError`] from [`ModManifest::validate`] or when the set has
/// a duplicate ID, a missing dependency, or a cycle.
pub fn validate_set(manifests: &[ModManifest]) -> Result<(), ManifestError> {
    let mut by_id: BTreeMap<&str, &ModManifest> = BTreeMap::new();
    for manifest in manifests {
        manifest.validate()?;
        if by_id.insert(manifest.id.as_str(), manifest).is_some() {
            return Err(ManifestError::DuplicateId {
                id: manifest.id.clone(),
            });
        }
    }

    for manifest in manifests {
        for dependency in &manifest.dependencies {
            if !by_id.contains_key(dependency.as_str()) {
                return Err(ManifestError::MissingDependency {
                    id: manifest.id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }
    detect_cycles(&by_id)
}

/// Check content registries for collisions and return the merged ID list.
///
/// Registries are accepted in load order. The returned IDs are sorted and
/// unique. An empty slice returns an empty list. This is a compatibility check,
/// not a merged registry: v0 has no combined lookup type, no override rules,
/// and no save migration.
///
/// # Errors
///
/// Returns [`ManifestError::DuplicateContentId`] when two registries define the
/// same ID.
pub fn merged_content(registries: &[ContentRegistry]) -> Result<Vec<String>, ManifestError> {
    let mut ids = BTreeSet::new();
    for registry in registries {
        for id in registry.ids() {
            if !ids.insert(id.to_owned()) {
                return Err(ManifestError::DuplicateContentId { id: id.to_owned() });
            }
        }
    }
    Ok(ids.into_iter().collect())
}

/// Errors returned while parsing or validating a mod manifest.
#[derive(Debug)]
pub enum ManifestError {
    /// A manifest did not parse as the expected v0 shape.
    Json {
        /// Underlying parser error.
        source: serde_json::Error,
    },
    /// A manifest uses a format version this build cannot read.
    UnsupportedFormatVersion {
        /// Version found in the manifest.
        found: u32,
    },
    /// A mod ID does not match the v0 namespace pattern.
    InvalidId {
        /// Rejected ID.
        id: String,
    },
    /// A dependency entry is not a valid mod ID.
    InvalidDependency {
        /// Manifest that owns the dependency list.
        id: String,
        /// Rejected dependency.
        dependency: String,
    },
    /// A required text field is empty or whitespace only.
    EmptyField {
        /// JSON field name.
        field: &'static str,
    },
    /// A mod version is not semver-like.
    InvalidVersion {
        /// Rejected version.
        version: String,
    },
    /// A content path is empty, absolute, or otherwise unsafe.
    InvalidPath {
        /// Content field name.
        field: &'static str,
        /// Rejected path.
        path: String,
        /// Short reason used in diagnostics.
        reason: &'static str,
    },
    /// A manifest lists the same dependency twice.
    DuplicateDependency {
        /// Manifest that owns the dependency list.
        id: String,
        /// Repeated dependency.
        dependency: String,
    },
    /// Two manifests in one set share an ID.
    DuplicateId {
        /// Conflicting ID.
        id: String,
    },
    /// A dependency is not present in the validated set.
    MissingDependency {
        /// Manifest that declares the dependency.
        id: String,
        /// Missing dependency.
        dependency: String,
    },
    /// Dependencies form a cycle within the validated set.
    DependencyCycle {
        /// Cycle in traversal order, with the repeated starting ID last.
        cycle: Vec<String>,
    },
    /// Two content registries define the same content ID.
    DuplicateContentId {
        /// Conflicting content ID.
        id: String,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { source } => write!(formatter, "invalid mod manifest JSON: {source}"),
            Self::UnsupportedFormatVersion { found } => write!(
                formatter,
                "unsupported mod manifest format version {found}; expected {MOD_FORMAT_VERSION}"
            ),
            Self::InvalidId { id } => write!(
                formatter,
                "mod id {id:?} must match ^[a-z0-9_]+(:[a-z0-9_]+)?$"
            ),
            Self::InvalidDependency { id, dependency } => write!(
                formatter,
                "mod {id} has invalid dependency id {dependency:?}"
            ),
            Self::EmptyField { field } => write!(formatter, "{field} must not be empty"),
            Self::InvalidVersion { version } => write!(
                formatter,
                "mod version {version:?} must be semver-like (MAJOR.MINOR.PATCH with an optional suffix)"
            ),
            Self::InvalidPath { field, path, reason } => {
                write!(formatter, "content.{field} path {path:?} {reason}")
            }
            Self::DuplicateDependency { id, dependency } => write!(
                formatter,
                "mod {id} lists dependency {dependency} more than once"
            ),
            Self::DuplicateId { id } => write!(formatter, "duplicate mod id {id}"),
            Self::MissingDependency { id, dependency } => {
                write!(formatter, "mod {id} requires missing mod {dependency}")
            }
            Self::DependencyCycle { cycle } => {
                write!(formatter, "dependency cycle: {}", cycle.join(" -> "))
            }
            Self::DuplicateContentId { id } => {
                write!(formatter, "duplicate content id {id} across mods")
            }
        }
    }
}

impl Error for ManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Done,
}

fn detect_cycles(manifests: &BTreeMap<&str, &ModManifest>) -> Result<(), ManifestError> {
    fn visit<'a>(
        id: &'a str,
        manifests: &BTreeMap<&'a str, &'a ModManifest>,
        states: &mut BTreeMap<&'a str, VisitState>,
        stack: &mut Vec<&'a str>,
    ) -> Result<(), ManifestError> {
        match states.get(id).copied() {
            Some(VisitState::Done) => return Ok(()),
            Some(VisitState::Visiting) => {
                let start = stack.iter().position(|entry| *entry == id).unwrap_or(0);
                let mut cycle: Vec<String> = stack[start..]
                    .iter()
                    .map(|entry| (*entry).to_owned())
                    .collect();
                cycle.push(id.to_owned());
                return Err(ManifestError::DependencyCycle { cycle });
            }
            None => {}
        }

        states.insert(id, VisitState::Visiting);
        stack.push(id);
        if let Some(&manifest) = manifests.get(id) {
            let mut dependencies: Vec<&str> =
                manifest.dependencies.iter().map(String::as_str).collect();
            dependencies.sort_unstable();
            for dependency in dependencies {
                visit(dependency, manifests, states, stack)?;
            }
        }
        stack.pop();
        states.insert(id, VisitState::Done);
        Ok(())
    }

    let mut states = BTreeMap::new();
    let mut stack = Vec::new();
    for id in manifests.keys().copied() {
        visit(id, manifests, &mut states, &mut stack)?;
    }
    Ok(())
}

/// Whether `id` matches `^[a-z0-9_]+(:[a-z0-9_]+)?$`.
fn is_valid_mod_id(id: &str) -> bool {
    let mut parts = id.split(':');
    let Some(namespace) = parts.next() else {
        return false;
    };
    if namespace.is_empty() || !namespace.bytes().all(is_id_byte) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(pack) => !pack.is_empty() && pack.bytes().all(is_id_byte) && parts.next().is_none(),
    }
}

fn is_id_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
}

/// Whether `version` is `MAJOR.MINOR.PATCH` with an optional `-`/`+` suffix.
fn is_semver_like(version: &str) -> bool {
    let (core, suffix) = match version.find(['-', '+']) {
        Some(index) => (&version[..index], Some(&version[index + 1..])),
        None => (version, None),
    };

    let mut parts = core.split('.');
    for _ in 0..3 {
        let Some(part) = parts.next() else {
            return false;
        };
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
    }
    if parts.next().is_some() {
        return false;
    }

    match suffix {
        None => true,
        Some(suffix) => {
            !suffix.is_empty()
                && !suffix.starts_with('.')
                && !suffix.ends_with('.')
                && !suffix.contains("..")
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
        }
    }
}

/// Validate a relative, forward-slash JSON path with no traversal.
fn check_content_path(path: &str) -> Result<(), &'static str> {
    if path.is_empty() {
        return Err("must not be empty");
    }
    if path.starts_with('/') {
        return Err("must be relative");
    }
    if path.contains('\\') {
        return Err("must use '/' separators");
    }
    if path.contains(':') {
        return Err("must not contain ':'");
    }
    if !path.ends_with(".json") {
        return Err("must end with .json");
    }
    for component in path.split('/') {
        if component.is_empty() {
            return Err("must not contain empty components");
        }
        if defines_traversal(component) {
            return Err("must not contain '.' or '..' components");
        }
        if !is_path_component(component) {
            return Err("must use ASCII letters, digits, '_', '-', and '.'");
        }
    }
    Ok(())
}

fn defines_traversal(component: &str) -> bool {
    component == "." || component == ".."
}

fn is_path_component(component: &str) -> bool {
    let mut bytes = component.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphanumeric() || matches!(first, b'_' | b'-'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    const EXAMPLE_MANIFEST: &str = include_str!("../../../content/mods/example.mod.json");

    fn manifest_value() -> Value {
        json!({
            "format_version": 1,
            "id": "example:pack",
            "name": "Example Pack",
            "version": "1.0.0",
            "game_version": ">=0.1.0 <0.2.0",
            "description": "Example content for tests.",
            "content": {
                "materials": "content/materials.json",
                "rooms": "content/rooms.json",
                "props": "content/props.json"
            },
            "dependencies": ["example:core", "example:library"],
            "draft": true
        })
    }

    fn parse(value: &Value) -> Result<ModManifest, ManifestError> {
        ModManifest::from_json_str(&serde_json::to_string(value).expect("manifest serializes"))
    }

    fn manifest_with(id: &str, dependencies: &[&str]) -> ModManifest {
        let mut value = manifest_value();
        value["id"] = json!(id);
        value["dependencies"] = json!(dependencies);
        parse(&value).expect("test manifest is valid")
    }

    fn registry(namespace: &str) -> ContentRegistry {
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
    fn valid_manifest_parses_and_exposes_fields() {
        let manifest = parse(&manifest_value()).expect("valid manifest");
        assert_eq!(manifest.format_version, MOD_FORMAT_VERSION);
        assert_eq!(manifest.id, "example:pack");
        assert_eq!(manifest.name, "Example Pack");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.game_version, ">=0.1.0 <0.2.0");
        assert_eq!(
            manifest.description.as_deref(),
            Some("Example content for tests.")
        );
        assert_eq!(manifest.content.materials, "content/materials.json");
        assert_eq!(manifest.content.rooms, "content/rooms.json");
        assert_eq!(manifest.content.props, "content/props.json");
        assert_eq!(manifest.dependencies, ["example:core", "example:library"]);
        assert!(manifest.draft);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn bare_namespace_ids_are_valid() {
        let manifest = manifest_with("mymod", &[]);
        assert_eq!(manifest.id, "mymod");
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn namespace_is_derived_from_the_manifest_id() {
        assert_eq!(manifest_with("mymod", &[]).namespace(), "mymod");
        assert_eq!(manifest_with("mymod:pack", &[]).namespace(), "mymod");
        assert_eq!(
            manifest_with("example:copper_age", &[]).namespace(),
            "example"
        );
    }

    #[test]
    fn example_mod_fixture_parses() {
        let manifest = ModManifest::from_json_str(EXAMPLE_MANIFEST).expect("example mod");
        assert_eq!(manifest.format_version, MOD_FORMAT_VERSION);
        assert_eq!(manifest.id, "example:copper_age");
        assert_eq!(manifest.checksum(), 0x7d90_7860_f7e7_d20d);
        assert!(manifest.draft);
        assert!(validate_set(&[manifest]).is_ok());
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for bad_id in [
            "",
            "Copper",
            "mymod:Pack",
            ":pack",
            "mymod:",
            "a:b:c",
            "my-mod",
            "mymod.pack",
            "mymod pack",
        ] {
            let mut value = manifest_value();
            value["id"] = json!(bad_id);
            let error = parse(&value).expect_err(bad_id);
            assert!(
                matches!(error, ManifestError::InvalidId { .. }),
                "{bad_id}: {error}"
            );
        }
    }

    #[test]
    fn empty_names_and_descriptions_are_rejected() {
        for empty in ["", "   "] {
            let mut value = manifest_value();
            value["name"] = json!(empty);
            let error = parse(&value).expect_err("empty name");
            assert!(matches!(error, ManifestError::EmptyField { field: "name" }));

            let mut value = manifest_value();
            value["description"] = json!(empty);
            let error = parse(&value).expect_err("empty description");
            assert!(matches!(
                error,
                ManifestError::EmptyField {
                    field: "description"
                }
            ));
        }
    }

    #[test]
    fn bad_versions_are_rejected() {
        for bad_version in [
            "",
            "   ",
            "1.0",
            "v1.0.0",
            "1.0.0-",
            "1.0.0.",
            "1.0.0 extra",
        ] {
            let mut value = manifest_value();
            value["version"] = json!(bad_version);
            let error = parse(&value).expect_err(bad_version);
            assert!(
                matches!(
                    error,
                    ManifestError::EmptyField { field: "version" }
                        | ManifestError::InvalidVersion { .. }
                ),
                "{bad_version}: {error}"
            );
        }

        for good_version in ["0.0.1", "12.34.56", "1.0.0-beta.1", "1.0.0+build.7"] {
            let mut value = manifest_value();
            value["version"] = json!(good_version);
            parse(&value).expect(good_version);
        }
    }

    #[test]
    fn game_version_must_not_be_empty() {
        for empty in ["", "   "] {
            let mut value = manifest_value();
            value["game_version"] = json!(empty);
            let error = parse(&value).expect_err("empty game version");
            assert!(matches!(
                error,
                ManifestError::EmptyField {
                    field: "game_version"
                }
            ));
        }
    }

    #[test]
    fn unsafe_content_paths_are_rejected() {
        for path in [
            "",
            "/content/materials.json",
            "content/../materials.json",
            "content\\materials.json",
            "C:/content/materials.json",
            "content/materials.txt",
            "content//materials.json",
            "./materials.json",
            "content/./materials.json",
            "~/materials.json",
        ] {
            let mut value = manifest_value();
            value["content"]["materials"] = json!(path);
            let error = parse(&value).expect_err(path);
            assert!(
                matches!(
                    error,
                    ManifestError::InvalidPath {
                        field: "materials",
                        ..
                    }
                ),
                "{path}: {error}"
            );
        }

        for good_path in [
            "materials.json",
            "content/materials.json",
            "content/v1/my-materials.json",
            "Mods/Example/content/materials.json",
        ] {
            let mut value = manifest_value();
            value["content"]["materials"] = json!(good_path);
            parse(&value).expect(good_path);
        }
    }

    #[test]
    fn duplicate_and_invalid_dependencies_are_rejected() {
        let mut value = manifest_value();
        value["dependencies"] = json!(["example:core", "example:core"]);
        let error = parse(&value).expect_err("duplicate dependency");
        assert!(matches!(error, ManifestError::DuplicateDependency { .. }));

        let mut value = manifest_value();
        value["dependencies"] = json!(["Example:Core"]);
        let error = parse(&value).expect_err("invalid dependency");
        assert!(matches!(error, ManifestError::InvalidDependency { .. }));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut value = manifest_value();
        value["entry"] = json!("scripts/main.lua");
        let error = parse(&value).expect_err("unknown field");
        assert!(matches!(error, ManifestError::Json { .. }));
    }

    #[test]
    fn optional_fields_may_be_omitted() {
        let mut value = manifest_value();
        value.as_object_mut().expect("object").remove("description");
        value
            .as_object_mut()
            .expect("object")
            .remove("dependencies");
        value.as_object_mut().expect("object").remove("draft");
        let manifest = parse(&value).expect("minimal manifest");
        assert!(manifest.description.is_none());
        assert!(manifest.dependencies.is_empty());
        assert!(!manifest.draft);
    }

    #[test]
    fn unsupported_format_versions_are_rejected() {
        let mut value = manifest_value();
        value["format_version"] = json!(2);
        let error = parse(&value).expect_err("bad version");
        assert!(matches!(
            error,
            ManifestError::UnsupportedFormatVersion { found: 2 }
        ));
    }

    #[test]
    fn checksum_is_stable_and_dependency_order_independent() {
        let manifest = parse(&manifest_value()).expect("valid manifest");
        let again = parse(&manifest_value()).expect("valid manifest");
        assert_eq!(manifest.checksum(), again.checksum());

        let mut reversed = manifest_value();
        reversed["dependencies"] = json!(["example:library", "example:core"]);
        let reversed = parse(&reversed).expect("valid manifest");
        assert_eq!(reversed.checksum(), manifest.checksum());

        let mut changed = manifest_value();
        changed["content"]["props"] = json!("content/v2/props.json");
        let changed = parse(&changed).expect("valid manifest");
        assert_ne!(changed.checksum(), manifest.checksum());
    }

    #[test]
    fn manifest_round_trips_through_serde() {
        let manifest = parse(&manifest_value()).expect("valid manifest");
        let encoded = serde_json::to_string(&manifest).expect("manifest serializes");
        let decoded = ModManifest::from_json_str(&encoded).expect("manifest parses again");
        assert_eq!(decoded, manifest);
        assert_eq!(decoded.checksum(), manifest.checksum());
    }

    #[test]
    fn empty_set_is_valid() {
        assert!(validate_set(&[]).is_ok());
        assert_eq!(
            merged_content(&[]).expect("empty merge"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn set_accepts_valid_dependencies() {
        let core = manifest_with("example:core", &[]);
        let library = manifest_with("example:library", &["example:core"]);
        let pack = manifest_with("example:pack", &["example:core", "example:library"]);
        let valid = [core, library, pack.clone()];
        validate_set(&valid).expect("valid set");
        assert!(pack.dependencies.contains(&"example:core".to_owned()));
    }

    #[test]
    fn duplicate_ids_are_rejected_in_a_set() {
        let first = manifest_with("example:pack", &[]);
        let second = manifest_with("example:pack", &[]);
        let error = validate_set(&[first, second]).expect_err("duplicate id");
        assert!(matches!(
            error,
            ManifestError::DuplicateId { id } if id == "example:pack"
        ));
    }

    #[test]
    fn missing_dependencies_are_rejected_in_a_set() {
        let pack = manifest_with("example:pack", &["example:missing"]);
        let error = validate_set(&[pack]).expect_err("missing dependency");
        assert!(matches!(
            error,
            ManifestError::MissingDependency { id, dependency }
                if id == "example:pack" && dependency == "example:missing"
        ));
    }

    #[test]
    fn dependency_cycles_are_rejected() {
        let first = manifest_with("example:a", &["example:b"]);
        let second = manifest_with("example:b", &["example:a"]);
        let error = validate_set(&[first, second]).expect_err("two-node cycle");
        match error {
            ManifestError::DependencyCycle { cycle } => {
                assert_eq!(cycle.first(), cycle.last());
                assert_eq!(cycle.len(), 3);
            }
            other => panic!("unexpected error: {other}"),
        }

        let only = manifest_with("example:self", &["example:self"]);
        let error = validate_set(&[only]).expect_err("self cycle");
        assert!(matches!(
            error,
            ManifestError::DependencyCycle { cycle }
                if cycle == ["example:self", "example:self"]
        ));
    }

    #[test]
    fn merged_content_returns_sorted_ids_and_rejects_collisions() {
        let base = ContentRegistry::base().expect("base content loads");
        let modded = registry("test");
        let merged = merged_content(&[base, modded]).expect("disjoint content");
        assert_eq!(merged.len(), 26);
        assert!(merged.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(merged.contains(&"test:material".to_owned()));

        let error = merged_content(&[registry("test"), registry("test")])
            .expect_err("duplicate content ids");
        assert!(matches!(
            error,
            ManifestError::DuplicateContentId { id } if id == "test:material"
        ));
    }

    #[test]
    fn validate_is_idempotent() {
        let manifest = parse(&manifest_value()).expect("valid manifest");
        manifest.validate().expect("first validation");
        manifest.validate().expect("second validation");
    }
}
