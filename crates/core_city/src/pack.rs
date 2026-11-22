//! In-memory mod content pack v0.
//!
//! A [`ContentPack`] binds one validated [`ModManifest`] to the material, room,
//! and prop documents it contributes. Construction is deliberately explicit:
//! [`ContentPack::from_json`] takes the four document bodies as strings, so the
//! same code path serves bundled fixtures, tests, an editor, and
//! [`ContentPack::from_dir`]. Directory loading only resolves the four files
//! named by the manifest; all content, namespace, and collision checks stay in
//! [`ContentPack::from_json`], so a pack loaded from disk and the same pack
//! loaded from memory have the same checksum.
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
//! # Directory loading
//!
//! [`ContentPack::from_dir`] reads one mod directory. The manifest is
//! `mod.json`; a single `*.mod.json` in the same directory is also accepted so
//! repositories can keep a named manifest. The three content paths are
//! resolved relative to the mod directory (for example
//! `content/materials.json`). Paths must stay under that directory after
//! canonicalization, so absolute paths, `..`, and symlinks that point outside
//! the pack are rejected. Each file must be UTF-8 and at most
//! [`MAX_PACK_FILE_BYTES`]; the manifest plus three documents are capped at
//! [`MAX_PACK_BYTES`].
//!
//! # What v0 does not do
//!
//! - It does not discover mods, read a mods root, download packages, unpack ZIP
//!   archives, or execute scripts. [`ContentPack::from_dir`] takes exactly one
//!   directory supplied by the caller.
//! - It does not watch the directory or hot reload a pack.
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
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::string::FromUtf8Error;

use crate::manifest::{ManifestError, ModManifest, MOD_FORMAT_VERSION};
use crate::registry::{ContentRegistry, Fnv1a, RegistryError};

/// Largest single file accepted by [`ContentPack::from_dir`], in bytes.
///
/// Manifests are normally a few kilobytes; the limit leaves room for generated
/// documents while keeping one broken pack from exhausting memory.
pub const MAX_PACK_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Largest combined size of a manifest and its three documents, in bytes.
///
/// Four documents at [`MAX_PACK_FILE_BYTES`] reach this cap exactly.
pub const MAX_PACK_BYTES: u64 = 8 * 1024 * 1024;

/// Canonical manifest file name inside a mod directory.
pub const MANIFEST_FILE_NAME: &str = "mod.json";

/// Repository-style manifest suffix accepted when `mod.json` is absent.
const NAMED_MANIFEST_SUFFIX: &str = ".mod.json";

/// A validated, in-memory mod content pack.
///
/// Build one with [`Self::from_json`] or [`Self::from_dir`]. The raw document
/// strings are retained for future packaging and save manifests; they are not
/// re-parsed. Fields are private so a pack cannot be mutated into an invalid
/// state.
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

    /// Load and validate one mod directory.
    ///
    /// The manifest is `<dir>/mod.json`, or exactly one `*.mod.json` in the
    /// same directory when `mod.json` is absent. The three content paths from
    /// the manifest are resolved relative to `dir`, then handed to
    /// [`Self::from_json`] unchanged, so namespace, duplicate, and base
    /// collision rules are identical to an in-memory load.
    ///
    /// # Security rules
    ///
    /// - Every path must be relative and may only contain normal components.
    ///   Absolute paths, `.`, `..`, and Windows prefix or `\` separators are
    ///   rejected before touching the filesystem.
    /// - The resolved file must stay under the canonical mod directory after
    ///   symlink resolution. A symlink that leaves the directory is rejected.
    /// - Every file must be a regular, UTF-8 file of at most
    ///   [`MAX_PACK_FILE_BYTES`]; the four files together may not exceed
    ///   [`MAX_PACK_BYTES`].
    ///
    /// This function does not discover mods, search parent directories, unpack
    /// archives, follow dependency or `dependencies` metadata, or watch for
    /// changes.
    ///
    /// # Errors
    ///
    /// Returns [`PackError::MissingManifest`], [`PackError::AmbiguousManifest`],
    /// [`PackError::NotADirectory`], [`PackError::MissingFile`],
    /// [`PackError::UnsafePath`], [`PackError::PathEscape`],
    /// [`PackError::NotAFile`], [`PackError::FileTooLarge`],
    /// [`PackError::PackTooLarge`], [`PackError::NotUtf8`], or
    /// [`PackError::Io`] for directory and file problems. Manifest and content
    /// problems are returned exactly as [`Self::from_json`] reports them.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// use core_city::pack::ContentPack;
    ///
    /// let pack = ContentPack::from_dir(Path::new("mods/example")).expect("valid pack");
    /// assert_eq!(pack.namespace(), "example");
    /// ```
    pub fn from_dir(dir: &Path) -> Result<Self, PackError> {
        let root = fs::canonicalize(dir).map_err(|source| PackError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        if !root.is_dir() {
            return Err(PackError::NotADirectory { path: root });
        }

        let manifest_name = find_manifest(&root)?;
        let mut budget = MAX_PACK_BYTES;
        let manifest_json = read_pack_file(&root, "manifest", &manifest_name, &mut budget)?;
        let manifest = ModManifest::from_json_str(&manifest_json).map_err(PackError::Manifest)?;

        let materials_json =
            read_pack_file(&root, "materials", &manifest.content.materials, &mut budget)?;
        let rooms_json = read_pack_file(&root, "rooms", &manifest.content.rooms, &mut budget)?;
        let props_json = read_pack_file(&root, "props", &manifest.content.props, &mut budget)?;

        Self::from_json(&manifest_json, &materials_json, &rooms_json, &props_json)
    }

    /// Load one mod directory and merge it with the bundled base content.
    ///
    /// Shorthand for [`Self::from_dir`] followed by
    /// [`Self::merged_with_base`]. The returned registry is validated and has
    /// the same deterministic checksum as the equivalent in-memory merge.
    ///
    /// # Errors
    ///
    /// Returns every error from [`Self::from_dir`] and
    /// [`Self::merged_with_base`].
    pub fn load_dir_with_base(dir: &Path) -> Result<ContentRegistry, PackError> {
        Self::from_dir(dir)?.merged_with_base()
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

/// Find `mod.json`, or exactly one `*.mod.json`, directly in `root`.
fn find_manifest(root: &Path) -> Result<String, PackError> {
    let canonical = root.join(MANIFEST_FILE_NAME);
    match fs::symlink_metadata(&canonical) {
        Ok(_) => return Ok(MANIFEST_FILE_NAME.to_owned()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(PackError::Io {
                path: canonical,
                source,
            });
        }
    }

    let entries = fs::read_dir(root).map_err(|source| PackError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| PackError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| PackError::Io {
            path: entry.path(),
            source,
        })?;
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.ends_with(NAMED_MANIFEST_SUFFIX) {
            manifests.push(name.to_owned());
        }
    }
    manifests.sort_unstable();

    match manifests.len() {
        0 => Err(PackError::MissingManifest {
            dir: root.to_path_buf(),
        }),
        1 => Ok(manifests.remove(0)),
        _ => Err(PackError::AmbiguousManifest {
            dir: root.to_path_buf(),
            manifests,
        }),
    }
}

/// Reject relative paths that could address anything outside the mod directory.
fn unsafe_relative_path(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return Some("must not be empty");
    }
    if path.contains('\\') {
        return Some("must use '/' separators");
    }
    if path.contains(':') {
        return Some("must not contain ':'");
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => return Some("must not contain '.' components"),
            Component::ParentDir => return Some("must not contain '..' components"),
            Component::RootDir | Component::Prefix(_) => return Some("must be relative"),
        }
    }
    None
}

/// Read one UTF-8 pack file, enforcing path, size, and total-budget rules.
fn read_pack_file(
    root: &Path,
    field: &'static str,
    relative: &str,
    budget: &mut u64,
) -> Result<String, PackError> {
    if let Some(reason) = unsafe_relative_path(relative) {
        return Err(PackError::UnsafePath {
            field,
            path: relative.to_owned(),
            reason,
        });
    }

    let candidate = root.join(relative);
    let resolved = fs::canonicalize(&candidate).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            PackError::MissingFile {
                field,
                path: candidate.clone(),
            }
        } else {
            PackError::Io {
                path: candidate.clone(),
                source,
            }
        }
    })?;
    if !resolved.starts_with(root) {
        return Err(PackError::PathEscape {
            field,
            path: candidate,
        });
    }

    let metadata = fs::metadata(&resolved).map_err(|source| PackError::Io {
        path: resolved.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(PackError::NotAFile {
            field,
            path: resolved,
        });
    }
    if metadata.len() > MAX_PACK_FILE_BYTES {
        return Err(PackError::FileTooLarge {
            field,
            path: resolved,
            bytes: metadata.len(),
            limit: MAX_PACK_FILE_BYTES,
        });
    }

    let file = File::open(&resolved).map_err(|source| PackError::Io {
        path: resolved.clone(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(MAX_PACK_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| PackError::Io {
            path: resolved.clone(),
            source,
        })?;
    let bytes_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if bytes_len > MAX_PACK_FILE_BYTES {
        return Err(PackError::FileTooLarge {
            field,
            path: resolved,
            bytes: bytes_len,
            limit: MAX_PACK_FILE_BYTES,
        });
    }
    if bytes_len > *budget {
        return Err(PackError::PackTooLarge {
            bytes: bytes_len,
            limit: MAX_PACK_BYTES,
        });
    }
    *budget -= bytes_len;

    String::from_utf8(bytes).map_err(|source| PackError::NotUtf8 {
        field,
        path: resolved,
        source,
    })
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
    /// A mod directory does not contain `mod.json` or a `*.mod.json` manifest.
    MissingManifest {
        /// Directory that was searched.
        dir: PathBuf,
    },
    /// A mod directory contains more than one candidate `*.mod.json` manifest.
    AmbiguousManifest {
        /// Directory that was searched.
        dir: PathBuf,
        /// Candidate manifest file names, sorted.
        manifests: Vec<String>,
    },
    /// The supplied path is not a directory.
    NotADirectory {
        /// Path that was supplied.
        path: PathBuf,
    },
    /// A file named by the manifest does not exist.
    MissingFile {
        /// Content field: `manifest`, `materials`, `rooms`, or `props`.
        field: &'static str,
        /// Resolved path that was attempted.
        path: PathBuf,
    },
    /// A path is absolute, traverses, or otherwise cannot stay inside the pack.
    UnsafePath {
        /// Content field: `manifest`, `materials`, `rooms`, or `props`.
        field: &'static str,
        /// Rejected path, relative to the mod directory.
        path: String,
        /// Short reason used in diagnostics.
        reason: &'static str,
    },
    /// A path resolved outside the mod directory, usually through a symlink.
    PathEscape {
        /// Content field: `manifest`, `materials`, `rooms`, or `props`.
        field: &'static str,
        /// Path before canonicalization.
        path: PathBuf,
    },
    /// A resolved path is not a regular file.
    NotAFile {
        /// Content field: `manifest`, `materials`, `rooms`, or `props`.
        field: &'static str,
        /// Resolved path.
        path: PathBuf,
    },
    /// One file exceeds [`MAX_PACK_FILE_BYTES`].
    FileTooLarge {
        /// Content field: `manifest`, `materials`, `rooms`, or `props`.
        field: &'static str,
        /// Resolved path.
        path: PathBuf,
        /// File size in bytes.
        bytes: u64,
        /// Per-file limit in bytes.
        limit: u64,
    },
    /// The manifest and content documents together exceed [`MAX_PACK_BYTES`].
    PackTooLarge {
        /// Size of the file that exceeded the remaining budget.
        bytes: u64,
        /// Total pack limit in bytes.
        limit: u64,
    },
    /// A pack file is not valid UTF-8.
    NotUtf8 {
        /// Content field: `manifest`, `materials`, `rooms`, or `props`.
        field: &'static str,
        /// Resolved path.
        path: PathBuf,
        /// Underlying conversion error.
        source: FromUtf8Error,
    },
    /// Reading the mod directory or one of its files failed.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
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
            Self::MissingManifest { dir } => write!(
                formatter,
                "mod directory {} has no {MANIFEST_FILE_NAME} or {NAMED_MANIFEST_SUFFIX} manifest",
                dir.display()
            ),
            Self::AmbiguousManifest { dir, manifests } => write!(
                formatter,
                "mod directory {} has {} candidate manifests ({})",
                dir.display(),
                manifests.len(),
                manifests.join(", ")
            ),
            Self::NotADirectory { path } => {
                write!(formatter, "mod path {} is not a directory", path.display())
            }
            Self::MissingFile { field, path } => write!(
                formatter,
                "content pack {field} file {} does not exist",
                path.display()
            ),
            Self::UnsafePath {
                field,
                path,
                reason,
            } => write!(formatter, "content pack {field} path {path:?} {reason}"),
            Self::PathEscape { field, path } => write!(
                formatter,
                "content pack {field} path {} escapes the mod directory",
                path.display()
            ),
            Self::NotAFile { field, path } => write!(
                formatter,
                "content pack {field} path {} is not a regular file",
                path.display()
            ),
            Self::FileTooLarge {
                field,
                path,
                bytes,
                limit,
            } => write!(
                formatter,
                "content pack {field} file {} is {bytes} bytes; limit is {limit}",
                path.display()
            ),
            Self::PackTooLarge { bytes, limit } => write!(
                formatter,
                "content pack needs {bytes} bytes; total limit is {limit}"
            ),
            Self::NotUtf8 {
                field,
                path,
                source,
            } => write!(
                formatter,
                "content pack {field} file {} is not valid UTF-8: {source}",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "content pack I/O failed at {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for PackError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Manifest(source) => Some(source),
            Self::Content(source) | Self::Base(source) | Self::Merge(source) => Some(source),
            Self::NotUtf8 { source, .. } => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Namespace { .. }
            | Self::CollisionWithBase { .. }
            | Self::ReservedNamespace { .. }
            | Self::MissingManifest { .. }
            | Self::AmbiguousManifest { .. }
            | Self::NotADirectory { .. }
            | Self::MissingFile { .. }
            | Self::UnsafePath { .. }
            | Self::PathEscape { .. }
            | Self::NotAFile { .. }
            | Self::FileTooLarge { .. }
            | Self::PackTooLarge { .. } => None,
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

    use std::sync::atomic::{AtomicU64, Ordering};

    /// Self-cleaning temporary mod directory. Unique per process and test.
    struct TempModDir {
        path: PathBuf,
    }

    impl TempModDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "townsmith-mod-pack-{}-{unique}-{label}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temp mod directory");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempModDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn write_file(root: &Path, relative: &str, contents: &str) {
        write_bytes(root, relative, contents.as_bytes());
    }

    fn write_bytes(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(path, contents).expect("write test file");
    }

    fn example_manifest_value(content: (&str, &str, &str)) -> Value {
        json!({
            "format_version": 1,
            "id": "example:copper_age",
            "name": "Copper Age example",
            "version": "0.1.0",
            "game_version": ">=0.1.0 <0.2.0",
            "description": "Example pack copied into a temporary directory.",
            "content": {
                "materials": content.0,
                "rooms": content.1,
                "props": content.2
            },
            "dependencies": [],
            "draft": true
        })
    }

    fn write_example_manifest(
        root: &Path,
        manifest_name: &str,
        content: (&str, &str, &str),
    ) -> String {
        let manifest = serde_json::to_string_pretty(&example_manifest_value(content))
            .expect("manifest serializes");
        write_file(root, manifest_name, &manifest);
        manifest
    }

    fn copy_example_content(root: &Path, prefix: &str) {
        for (name, body) in [
            ("materials.json", EXAMPLE_MATERIALS),
            ("rooms.json", EXAMPLE_ROOMS),
            ("props.json", EXAMPLE_PROPS),
        ] {
            write_file(root, &format!("{prefix}{name}"), body);
        }
    }

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

    #[test]
    fn from_dir_loads_example_copy_with_the_in_memory_checksum() {
        let dir = TempModDir::new("valid");
        let manifest = write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        copy_example_content(dir.path(), "");

        let from_disk = ContentPack::from_dir(dir.path()).expect("pack loads from disk");
        let in_memory =
            ContentPack::from_json(&manifest, EXAMPLE_MATERIALS, EXAMPLE_ROOMS, EXAMPLE_PROPS)
                .expect("pack loads in memory");
        assert_eq!(from_disk, in_memory);
        assert_eq!(from_disk.checksum(), in_memory.checksum());
        assert_eq!(from_disk.namespace(), "example");
        assert_eq!(from_disk.manifest().id, "example:copper_age");
        assert_eq!(from_disk.registry().material_count(), 3);
        assert_eq!(from_disk.registry().room_count(), 2);
        assert_eq!(from_disk.registry().prop_count(), 3);
        assert_eq!(from_disk.registry().len(), 8);
        assert!(from_disk.materials_json().contains("example:copper_sheet"));
        assert!(from_disk.rooms_json().contains("example:smelter_hall"));
        assert!(from_disk.props_json().contains("example:copper_smelter"));

        let merged = ContentPack::load_dir_with_base(dir.path()).expect("merge succeeds");
        assert_eq!(merged.len(), 31);
        assert_eq!(merged.checksum(), 0x330a_538d_0780_5b50);
        assert!(merged.material("example:copper_sheet").is_some());
    }

    #[test]
    fn from_dir_resolves_nested_content_and_named_manifest() {
        let dir = TempModDir::new("nested");
        write_example_manifest(
            dir.path(),
            "example.mod.json",
            (
                "content/materials.json",
                "content/rooms.json",
                "content/props.json",
            ),
        );
        copy_example_content(dir.path(), "content/");

        let pack = ContentPack::from_dir(dir.path()).expect("nested pack loads");
        assert_eq!(pack.registry().len(), 8);
        assert_eq!(pack.namespace(), "example");
    }

    #[test]
    fn mod_json_takes_precedence_over_named_manifests() {
        let dir = TempModDir::new("precedence");
        write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        write_file(dir.path(), "broken.mod.json", "{");
        copy_example_content(dir.path(), "");

        let pack = ContentPack::from_dir(dir.path()).expect("mod.json wins");
        assert_eq!(pack.registry().len(), 8);
    }

    #[test]
    fn missing_manifest_is_reported() {
        let dir = TempModDir::new("missing-manifest");
        copy_example_content(dir.path(), "");

        let error = ContentPack::from_dir(dir.path()).expect_err("no manifest");
        assert!(
            matches!(error, PackError::MissingManifest { .. }),
            "{error}"
        );
    }

    #[test]
    fn ambiguous_named_manifests_are_rejected() {
        let dir = TempModDir::new("ambiguous");
        write_example_manifest(
            dir.path(),
            "one.mod.json",
            ("materials.json", "rooms.json", "props.json"),
        );
        write_example_manifest(
            dir.path(),
            "two.mod.json",
            ("materials.json", "rooms.json", "props.json"),
        );

        let error = ContentPack::from_dir(dir.path()).expect_err("two manifests");
        match error {
            PackError::AmbiguousManifest { manifests, .. } => {
                assert_eq!(manifests, ["one.mod.json", "two.mod.json"]);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn missing_content_file_is_reported() {
        let dir = TempModDir::new("missing-content");
        write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        write_file(dir.path(), "rooms.json", EXAMPLE_ROOMS);
        write_file(dir.path(), "props.json", EXAMPLE_PROPS);

        let error = ContentPack::from_dir(dir.path()).expect_err("missing materials");
        assert!(
            matches!(
                error,
                PackError::MissingFile { field: "materials", ref path }
                    if path.ends_with("materials.json")
            ),
            "{error}"
        );
    }

    #[test]
    fn directory_in_place_of_content_file_is_rejected() {
        let dir = TempModDir::new("content-dir");
        write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        fs::create_dir(dir.path().join("materials.json")).expect("create directory");
        write_file(dir.path(), "rooms.json", EXAMPLE_ROOMS);
        write_file(dir.path(), "props.json", EXAMPLE_PROPS);

        let error = ContentPack::from_dir(dir.path()).expect_err("materials is a directory");
        assert!(
            matches!(
                error,
                PackError::NotAFile {
                    field: "materials",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn manifest_path_traversal_and_absolute_paths_are_rejected() {
        for bad_path in [
            "../materials.json",
            "/etc/materials.json",
            "content/../../materials.json",
            "C:/materials.json",
        ] {
            let dir = TempModDir::new("unsafe-manifest");
            write_example_manifest(
                dir.path(),
                MANIFEST_FILE_NAME,
                (bad_path, "rooms.json", "props.json"),
            );
            copy_example_content(dir.path(), "");

            let error = ContentPack::from_dir(dir.path()).expect_err(bad_path);
            assert!(
                matches!(
                    error,
                    PackError::Manifest(ManifestError::InvalidPath {
                        field: "materials",
                        ..
                    })
                ),
                "{bad_path}: {error}"
            );
        }
    }

    #[test]
    fn loader_rejects_unsafe_paths_before_resolution() {
        let root = Path::new("/nonexistent-townsmith-mod");
        for bad_path in [
            "",
            "/etc/passwd",
            "../outside.json",
            "./materials.json",
            "content/../props.json",
            "content\\props.json",
            "C:/props.json",
        ] {
            let mut budget = MAX_PACK_BYTES;
            let error =
                read_pack_file(root, "materials", bad_path, &mut budget).expect_err(bad_path);
            assert!(
                matches!(
                    error,
                    PackError::UnsafePath {
                        field: "materials",
                        ..
                    }
                ),
                "{bad_path}: {error}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = TempModDir::new("symlink");
        let outside = TempModDir::new("outside");
        write_file(outside.path(), "materials.json", EXAMPLE_MATERIALS);
        write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        write_file(dir.path(), "rooms.json", EXAMPLE_ROOMS);
        write_file(dir.path(), "props.json", EXAMPLE_PROPS);
        symlink(
            outside.path().join("materials.json"),
            dir.path().join("materials.json"),
        )
        .expect("create symlink");

        let error = ContentPack::from_dir(dir.path()).expect_err("symlink escape");
        assert!(
            matches!(
                error,
                PackError::PathEscape {
                    field: "materials",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn oversize_file_is_rejected_before_parsing() {
        let dir = TempModDir::new("oversize");
        write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        write_file(dir.path(), "rooms.json", EXAMPLE_ROOMS);
        write_file(dir.path(), "props.json", EXAMPLE_PROPS);
        let mut oversized = String::from("{\"format_version\":1,\"materials\":[]}");
        oversized
            .push_str(&" ".repeat(usize::try_from(MAX_PACK_FILE_BYTES).expect("limit fits usize")));
        write_file(dir.path(), "materials.json", &oversized);

        let error = ContentPack::from_dir(dir.path()).expect_err("oversize file");
        assert!(
            matches!(
                error,
                PackError::FileTooLarge {
                    field: "materials",
                    bytes,
                    limit,
                    ..
                } if bytes > limit && limit == MAX_PACK_FILE_BYTES
            ),
            "{error}"
        );
    }

    #[test]
    fn total_budget_is_enforced() {
        let dir = TempModDir::new("budget");
        write_file(dir.path(), "materials.json", "{}");

        let mut budget = 1;
        let error =
            read_pack_file(dir.path(), "materials", "materials.json", &mut budget).unwrap_err();
        assert!(
            matches!(
                error,
                PackError::PackTooLarge {
                    bytes: 2,
                    limit: MAX_PACK_BYTES
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn malformed_content_json_is_reported_as_content_error() {
        let dir = TempModDir::new("malformed");
        write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        write_file(dir.path(), "materials.json", "{");
        write_file(dir.path(), "rooms.json", EXAMPLE_ROOMS);
        write_file(dir.path(), "props.json", EXAMPLE_PROPS);

        let error = ContentPack::from_dir(dir.path()).expect_err("malformed materials");
        assert!(
            matches!(error, PackError::Content(RegistryError::Json { .. })),
            "{error}"
        );
    }

    #[test]
    fn namespace_mismatch_is_rejected_from_directory() {
        let dir = TempModDir::new("namespace");
        write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        write_file(
            dir.path(),
            "materials.json",
            &EXAMPLE_MATERIALS.replace("example:", "other:"),
        );
        write_file(dir.path(), "rooms.json", EXAMPLE_ROOMS);
        write_file(dir.path(), "props.json", EXAMPLE_PROPS);

        let error = ContentPack::from_dir(dir.path()).expect_err("wrong namespace");
        assert!(
            matches!(
                error,
                PackError::Namespace { ref namespace, ref id }
                    if namespace == "example" && id.starts_with("other:")
            ),
            "{error}"
        );
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let dir = TempModDir::new("utf8");
        write_example_manifest(
            dir.path(),
            MANIFEST_FILE_NAME,
            ("materials.json", "rooms.json", "props.json"),
        );
        write_bytes(dir.path(), "materials.json", &[b'{', 0xff, b'}']);
        write_file(dir.path(), "rooms.json", EXAMPLE_ROOMS);
        write_file(dir.path(), "props.json", EXAMPLE_PROPS);

        let error = ContentPack::from_dir(dir.path()).expect_err("invalid UTF-8");
        assert!(
            matches!(
                error,
                PackError::NotUtf8 {
                    field: "materials",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn file_path_instead_of_directory_is_rejected() {
        let dir = TempModDir::new("not-a-dir");
        write_file(dir.path(), "manifest.json", EXAMPLE_MANIFEST);
        let file = dir.path().join("manifest.json");

        let error = ContentPack::from_dir(&file).expect_err("not a directory");
        assert!(matches!(error, PackError::NotADirectory { .. }), "{error}");
    }
}
