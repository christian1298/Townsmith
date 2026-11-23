//! Content-addressed local `.tsbp` library.
//!
//! The library stores validated packages under an explicit absolute directory:
//!
//! ```text
//! <root>/<first-two-hash-hex>/<hash-hex>.tsbp
//! ```
//!
//! The address is the package's validated `blake3:<hex>` content hash. The
//! `blake3:` prefix is omitted from disk names so the layout is valid on
//! Windows. A package is written to a temporary file in the target shard and
//! published with a hard link, which is atomic and never overwrites a
//! different file. Inserting bytes that are already stored under the same hash
//! is a no-op; different bytes under an existing hash are rejected.
//!
//! There is no discovery, download, Workshop/ZIP packaging, script execution,
//! or save integration here. The world save still stores absolute package
//! paths; replacing them with library references is a later slice.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::BlueprintError;
use crate::limits::MAX_PACKAGE_BYTES;
use crate::package::load_tsbp_bytes;

const HASH_PREFIX: &str = "blake3:";
const HASH_HEX_LEN: usize = 64;
const LIBRARY_DOMAIN: &[u8] = b"townsmith.tsbp.library.v1\0";

/// One stored package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryEntry {
    /// Canonical `blake3:<hex>` content hash.
    pub hash: String,
    /// Canonical path to the stored package.
    pub path: PathBuf,
    /// Package size in bytes.
    pub bytes: u64,
}

/// Deterministic library totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LibrarySummary {
    /// Number of stored packages.
    pub count: u64,
    /// Sum of the stored package sizes in bytes.
    pub total_bytes: u64,
    /// Checksum over the sorted package hashes and sizes.
    ///
    /// This is a catalog identity helper, not a cryptographic hash of the
    /// package bytes. It is stable across insertion order and process runs.
    pub checksum: u64,
}

/// Result of one [`BlueprintLibrary::insert_file`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryInsert {
    /// Canonical `blake3:<hex>` content hash.
    pub hash: String,
    /// Canonical path to the stored package.
    pub path: PathBuf,
    /// `false` when identical bytes were already stored under this hash.
    pub inserted: bool,
}

/// A validated, content-addressed local blueprint library.
#[derive(Debug, Clone)]
pub struct BlueprintLibrary {
    root: PathBuf,
}

impl BlueprintLibrary {
    /// Open an existing absolute root directory.
    ///
    /// The root is canonicalized once. It must already exist, be a directory,
    /// and contain no `.` or `..` components. The library does not create or
    /// discover the root.
    ///
    /// # Errors
    ///
    /// Returns [`BlueprintLibraryError::InvalidRoot`] for an empty, relative,
    /// or traversing path, [`BlueprintLibraryError::RootMissing`] when the root
    /// does not exist, [`BlueprintLibraryError::RootNotDirectory`] when it is
    /// not a directory, and [`BlueprintLibraryError::Io`] for other filesystem
    /// failures.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, BlueprintLibraryError> {
        let path = root.into();
        if path.as_os_str().is_empty() {
            return Err(BlueprintLibraryError::InvalidRoot {
                path,
                reason: "must not be empty",
            });
        }
        if !path.is_absolute() {
            return Err(BlueprintLibraryError::InvalidRoot {
                path,
                reason: "must be absolute",
            });
        }
        if let Some(reason) = path_component_error(&path) {
            return Err(BlueprintLibraryError::InvalidRoot { path, reason });
        }
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(BlueprintLibraryError::RootMissing { path });
            }
            Err(source) => return Err(BlueprintLibraryError::Io { path, source }),
        };
        if !metadata.is_dir() {
            return Err(BlueprintLibraryError::RootNotDirectory { path });
        }
        let root =
            fs::canonicalize(&path).map_err(|source| BlueprintLibraryError::Io { path, source })?;
        Ok(Self { root })
    }

    /// Canonical root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate a canonical `blake3:<64 lowercase hex>` hash.
    ///
    /// This rejects empty strings, relative or absolute paths, traversal,
    /// uppercase hex, and any other shape that cannot name a library entry.
    pub fn validate_hash(hash: &str) -> Result<(), BlueprintLibraryError> {
        parse_hash(hash).map(|_| ())
    }

    /// Insert one `.tsbp` file.
    ///
    /// The source must be an absolute `.tsbp` path (case-insensitive
    /// extension) without `.` or `..` components. Its bytes are read once with
    /// a [`MAX_PACKAGE_BYTES`] cap and validated by the existing package
    /// loader. The validated content hash selects the destination.
    ///
    /// Identical bytes already stored under the hash return
    /// [`LibraryInsert::inserted`] as `false`. A hash that already maps to
    /// different bytes is a collision and is rejected. A failed insert leaves
    /// the library unchanged apart from a removed empty shard directory.
    ///
    /// # Errors
    ///
    /// Returns [`BlueprintLibraryError::InvalidPath`],
    /// [`BlueprintLibraryError::MissingFile`],
    /// [`BlueprintLibraryError::NotAFile`],
    /// [`BlueprintLibraryError::TooLarge`],
    /// [`BlueprintLibraryError::InvalidPackage`],
    /// [`BlueprintLibraryError::PathEscape`] for a symlinked shard or entry,
    /// [`BlueprintLibraryError::Collision`], or [`BlueprintLibraryError::Io`].
    pub fn insert_file(&self, path: &Path) -> Result<LibraryInsert, BlueprintLibraryError> {
        validate_source_path(path)?;
        let bytes = read_source_file(path)?;
        self.insert_validated_bytes(path, &bytes)
    }

    /// Resolve a hash to its stored package path.
    ///
    /// Invalid hashes, missing entries, symlinked entries, and entries outside
    /// the canonical root all return `None`. Use
    /// [`Self::resolve_checked`] when the caller needs the failure reason.
    #[must_use]
    pub fn resolve(&self, hash: &str) -> Option<PathBuf> {
        self.resolve_checked(hash).ok().flatten()
    }

    /// Resolve a hash, distinguishing invalid or unsafe entries from a miss.
    ///
    /// # Errors
    ///
    /// Returns [`BlueprintLibraryError::InvalidHash`] for a malformed hash,
    /// [`BlueprintLibraryError::PathEscape`] when a matching path leaves the
    /// canonical root or is a symlink, and [`BlueprintLibraryError::Io`] for
    /// other filesystem failures. A valid hash that is not stored returns
    /// `Ok(None)`.
    pub fn resolve_checked(&self, hash: &str) -> Result<Option<PathBuf>, BlueprintLibraryError> {
        let hex = parse_hash(hash)?;
        let shard_path = self.root.join(&hex[..2]);
        let expected = format!("{hex}.tsbp");
        let target = shard_path.join(&expected);
        let metadata = match fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(BlueprintLibraryError::Io {
                    path: target,
                    source,
                })
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(BlueprintLibraryError::PathEscape { path: target });
        }
        if !metadata.is_file() {
            return Err(BlueprintLibraryError::NotAFile { path: target });
        }
        let canonical = fs::canonicalize(&target).map_err(|source| BlueprintLibraryError::Io {
            path: target.clone(),
            source,
        })?;
        let shard = fs::canonicalize(&shard_path).map_err(|source| BlueprintLibraryError::Io {
            path: shard_path,
            source,
        })?;
        if canonical.parent() != Some(shard.as_path())
            || canonical.file_name().and_then(|name| name.to_str()) != Some(expected.as_str())
            || shard.parent() != Some(self.root.as_path())
        {
            return Err(BlueprintLibraryError::PathEscape { path: target });
        }
        Ok(Some(canonical))
    }

    /// Whether a valid hash resolves to a stored package.
    #[must_use]
    pub fn contains(&self, hash: &str) -> bool {
        self.resolve(hash).is_some()
    }

    /// Deterministic package list, sorted by canonical hash.
    ///
    /// Only `<root>/<2 lowercase hex>/<64 lowercase hex>.tsbp` regular files
    /// are returned. Unknown files, directory shards that are symlinks, and
    /// non-matching names are ignored. A readable library with no packages
    /// returns an empty vector.
    ///
    /// # Errors
    ///
    /// Returns [`BlueprintLibraryError::Io`] when the root or a matching shard
    /// cannot be read or canonicalized.
    pub fn list(&self) -> Result<Vec<LibraryEntry>, BlueprintLibraryError> {
        let directory = fs::read_dir(&self.root).map_err(|source| BlueprintLibraryError::Io {
            path: self.root.clone(),
            source,
        })?;
        let mut entries = Vec::new();
        for entry in directory {
            let entry = entry.map_err(|source| BlueprintLibraryError::Io {
                path: self.root.clone(),
                source,
            })?;
            let shard_name = entry.file_name();
            let Some(shard_name) = shard_name.to_str() else {
                continue;
            };
            if !is_hash_prefix(shard_name) {
                continue;
            }
            let shard_path = entry.path();
            let metadata = match fs::symlink_metadata(&shard_path) {
                Ok(metadata) => metadata,
                Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(BlueprintLibraryError::Io {
                        path: shard_path,
                        source,
                    });
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            let shard =
                fs::canonicalize(&shard_path).map_err(|source| BlueprintLibraryError::Io {
                    path: shard_path.clone(),
                    source,
                })?;
            if shard.parent() != Some(self.root.as_path()) {
                continue;
            }
            let files = fs::read_dir(&shard).map_err(|source| BlueprintLibraryError::Io {
                path: shard.clone(),
                source,
            })?;
            for file in files {
                let file = file.map_err(|source| BlueprintLibraryError::Io {
                    path: shard.clone(),
                    source,
                })?;
                let name = file.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let Some(hex) = name.strip_suffix(".tsbp") else {
                    continue;
                };
                if !is_hash_hex(hex) || &hex[..2] != shard_name {
                    continue;
                }
                let file_path = file.path();
                let metadata = match fs::symlink_metadata(&file_path) {
                    Ok(metadata) => metadata,
                    Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
                    Err(source) => {
                        return Err(BlueprintLibraryError::Io {
                            path: file_path,
                            source,
                        });
                    }
                };
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    continue;
                }
                let canonical =
                    fs::canonicalize(&file_path).map_err(|source| BlueprintLibraryError::Io {
                        path: file_path.clone(),
                        source,
                    })?;
                if canonical.parent() != Some(shard.as_path()) {
                    continue;
                }
                entries.push(LibraryEntry {
                    hash: format!("{HASH_PREFIX}{hex}"),
                    path: canonical,
                    bytes: metadata.len(),
                });
            }
        }
        entries.sort_by(|left, right| left.hash.cmp(&right.hash));
        Ok(entries)
    }

    /// Summarize the stored packages.
    ///
    /// The checksum covers the sorted `blake3:<hex>` hashes and package sizes
    /// under a domain tag, so it does not depend on insertion order.
    ///
    /// # Errors
    ///
    /// Returns any error from [`Self::list`].
    pub fn summary(&self) -> Result<LibrarySummary, BlueprintLibraryError> {
        let entries = self.list()?;
        let count = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        let total_bytes = entries.iter().map(|entry| entry.bytes).sum();
        let mut hasher = blake3::Hasher::new();
        hasher.update(LIBRARY_DOMAIN);
        for entry in &entries {
            hasher.update(entry.hash.as_bytes());
            hasher.update(&entry.bytes.to_le_bytes());
        }
        let digest = hasher.finalize();
        let mut checksum_bytes = [0u8; 8];
        checksum_bytes.copy_from_slice(&digest.as_bytes()[..8]);
        Ok(LibrarySummary {
            count,
            total_bytes,
            checksum: u64::from_be_bytes(checksum_bytes),
        })
    }

    fn insert_validated_bytes(
        &self,
        source: &Path,
        bytes: &[u8],
    ) -> Result<LibraryInsert, BlueprintLibraryError> {
        let package = load_tsbp_bytes(bytes).map_err(|source_error| {
            BlueprintLibraryError::InvalidPackage {
                path: source.to_path_buf(),
                source: source_error,
            }
        })?;
        let hash = package.metadata.content_hash;
        let hex = parse_hash(&hash)?;
        let shard_name = &hex[..2];
        let shard_path = self.root.join(shard_name);
        let (shard, created_shard) = self.open_shard(&shard_path)?;
        let expected = format!("{hex}.tsbp");
        let target = shard.join(&expected);

        match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                let result = existing_entry(&target, &metadata, &hash, bytes);
                if result.is_err() {
                    cleanup_empty_shard(&shard, created_shard);
                }
                return result;
            }
            Err(source_error) if source_error.kind() == io::ErrorKind::NotFound => {}
            Err(source_error) => {
                cleanup_empty_shard(&shard, created_shard);
                return Err(BlueprintLibraryError::Io {
                    path: target,
                    source: source_error,
                });
            }
        }

        let temp = match write_temp_package(&shard, bytes) {
            Ok(temp) => temp,
            Err(error) => {
                cleanup_empty_shard(&shard, created_shard);
                return Err(error);
            }
        };
        // `rename` would overwrite an existing destination on Unix. A hard
        // link is atomic and fails with AlreadyExists, so different bytes can
        // never replace the stored package; the temporary name is unlinked
        // immediately after publication.
        let link = fs::hard_link(&temp, &target);
        let _ = fs::remove_file(&temp);
        match link {
            Ok(()) => Ok(LibraryInsert {
                hash,
                path: target,
                inserted: true,
            }),
            Err(source_error) if source_error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&target).map_err(|source_error| {
                    BlueprintLibraryError::Io {
                        path: target.clone(),
                        source: source_error,
                    }
                })?;
                let result = existing_entry(&target, &metadata, &hash, bytes);
                cleanup_empty_shard(&shard, created_shard);
                result
            }
            Err(source_error) => {
                cleanup_empty_shard(&shard, created_shard);
                Err(BlueprintLibraryError::Io {
                    path: target,
                    source: source_error,
                })
            }
        }
    }

    fn open_shard(&self, shard_path: &Path) -> Result<(PathBuf, bool), BlueprintLibraryError> {
        match fs::symlink_metadata(shard_path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(BlueprintLibraryError::PathEscape {
                        path: shard_path.to_path_buf(),
                    });
                }
                let shard =
                    fs::canonicalize(shard_path).map_err(|source| BlueprintLibraryError::Io {
                        path: shard_path.to_path_buf(),
                        source,
                    })?;
                if shard.parent() != Some(self.root.as_path()) {
                    return Err(BlueprintLibraryError::PathEscape {
                        path: shard_path.to_path_buf(),
                    });
                }
                Ok((shard, false))
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(shard_path) {
                    Ok(()) => {}
                    Err(source_error) if source_error.kind() == io::ErrorKind::AlreadyExists => {
                        return self.open_shard(shard_path);
                    }
                    Err(source_error) => {
                        return Err(BlueprintLibraryError::Io {
                            path: shard_path.to_path_buf(),
                            source: source_error,
                        });
                    }
                }
                let shard =
                    fs::canonicalize(shard_path).map_err(|source| BlueprintLibraryError::Io {
                        path: shard_path.to_path_buf(),
                        source,
                    })?;
                if shard.parent() != Some(self.root.as_path()) {
                    cleanup_empty_shard(&shard, true);
                    return Err(BlueprintLibraryError::PathEscape {
                        path: shard_path.to_path_buf(),
                    });
                }
                Ok((shard, true))
            }
            Err(source) => Err(BlueprintLibraryError::Io {
                path: shard_path.to_path_buf(),
                source,
            }),
        }
    }
}

/// Errors returned by the local blueprint library.
#[derive(Debug)]
pub enum BlueprintLibraryError {
    /// The library root is empty, relative, or has unsafe components.
    InvalidRoot {
        /// Supplied root.
        path: PathBuf,
        /// Why the root was rejected.
        reason: &'static str,
    },
    /// The library root does not exist.
    RootMissing {
        /// Supplied root.
        path: PathBuf,
    },
    /// The library root is not a directory.
    RootNotDirectory {
        /// Supplied root.
        path: PathBuf,
    },
    /// A hash is not canonical `blake3:<64 lowercase hex>`.
    InvalidHash {
        /// Supplied hash.
        hash: String,
        /// Why the hash was rejected.
        reason: &'static str,
    },
    /// A source path is empty, relative, traversing, or not `.tsbp`.
    InvalidPath {
        /// Supplied source path.
        path: PathBuf,
        /// Why the path was rejected.
        reason: &'static str,
    },
    /// The source package does not exist.
    MissingFile {
        /// Supplied source path.
        path: PathBuf,
    },
    /// The source path or stored entry is not a regular file.
    NotAFile {
        /// Offending path.
        path: PathBuf,
    },
    /// The package failed existing `.tsbp` validation.
    InvalidPackage {
        /// Supplied source path.
        path: PathBuf,
        /// Package validation error.
        source: BlueprintError,
    },
    /// The package exceeds [`MAX_PACKAGE_BYTES`].
    TooLarge {
        /// Offending path.
        path: PathBuf,
        /// Observed size.
        bytes: u64,
        /// Library limit.
        limit: u64,
    },
    /// A path leaves the canonical library root, usually through a symlink.
    PathEscape {
        /// Offending path.
        path: PathBuf,
    },
    /// The hash already maps to different bytes.
    Collision {
        /// Colliding canonical hash.
        hash: String,
        /// Existing stored path.
        path: PathBuf,
    },
    /// Filesystem error.
    Io {
        /// Offending path.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
}

impl std::fmt::Display for BlueprintLibraryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRoot { path, reason } => {
                write!(
                    formatter,
                    "blueprint library root {} is invalid: {reason}",
                    path.display()
                )
            }
            Self::RootMissing { path } => {
                write!(
                    formatter,
                    "blueprint library root {} does not exist",
                    path.display()
                )
            }
            Self::RootNotDirectory { path } => {
                write!(
                    formatter,
                    "blueprint library root {} is not a directory",
                    path.display()
                )
            }
            Self::InvalidHash { hash, reason } => {
                write!(
                    formatter,
                    "blueprint library hash {hash:?} is invalid: {reason}"
                )
            }
            Self::InvalidPath { path, reason } => {
                write!(
                    formatter,
                    "blueprint library source {} is invalid: {reason}",
                    path.display()
                )
            }
            Self::MissingFile { path } => {
                write!(
                    formatter,
                    "blueprint library source {} does not exist",
                    path.display()
                )
            }
            Self::NotAFile { path } => {
                write!(
                    formatter,
                    "blueprint library path {} is not a file",
                    path.display()
                )
            }
            Self::InvalidPackage { path, source } => {
                write!(
                    formatter,
                    "blueprint library package {} is invalid: {source}",
                    path.display()
                )
            }
            Self::TooLarge { path, bytes, limit } => write!(
                formatter,
                "blueprint library package {} is {bytes} bytes, limit is {limit}",
                path.display()
            ),
            Self::PathEscape { path } => write!(
                formatter,
                "blueprint library path {} leaves the library root",
                path.display()
            ),
            Self::Collision { hash, path } => write!(
                formatter,
                "blueprint library hash {hash} already maps to different bytes at {}",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "blueprint library I/O error at {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for BlueprintLibraryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidPackage { source, .. } => Some(source),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn parse_hash(hash: &str) -> Result<&str, BlueprintLibraryError> {
    let Some(hex) = hash.strip_prefix(HASH_PREFIX) else {
        return Err(BlueprintLibraryError::InvalidHash {
            hash: hash.to_string(),
            reason: "must start with 'blake3:'",
        });
    };
    if hex.len() != HASH_HEX_LEN {
        return Err(BlueprintLibraryError::InvalidHash {
            hash: hash.to_string(),
            reason: "must contain exactly 64 hex digits",
        });
    }
    if !is_hash_hex(hex) {
        return Err(BlueprintLibraryError::InvalidHash {
            hash: hash.to_string(),
            reason: "must contain lowercase hex digits only",
        });
    }
    Ok(hex)
}

fn is_hash_hex(value: &str) -> bool {
    value.len() == HASH_HEX_LEN
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_hash_prefix(value: &str) -> bool {
    value.len() == 2
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn path_component_error(path: &Path) -> Option<&'static str> {
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::RootDir | Component::Prefix(_) => {}
            Component::CurDir => return Some("must not contain '.' components"),
            Component::ParentDir => return Some("must not contain '..' components"),
        }
    }
    None
}

fn validate_source_path(path: &Path) -> Result<(), BlueprintLibraryError> {
    if path.as_os_str().is_empty() {
        return Err(BlueprintLibraryError::InvalidPath {
            path: path.to_path_buf(),
            reason: "must not be empty",
        });
    }
    if !path.is_absolute() {
        return Err(BlueprintLibraryError::InvalidPath {
            path: path.to_path_buf(),
            reason: "must be absolute",
        });
    }
    if let Some(reason) = path_component_error(path) {
        return Err(BlueprintLibraryError::InvalidPath {
            path: path.to_path_buf(),
            reason,
        });
    }
    let extension_ok = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tsbp"));
    if !extension_ok {
        return Err(BlueprintLibraryError::InvalidPath {
            path: path.to_path_buf(),
            reason: "must end with .tsbp",
        });
    }
    Ok(())
}

fn read_source_file(path: &Path) -> Result<Vec<u8>, BlueprintLibraryError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Err(BlueprintLibraryError::MissingFile {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(BlueprintLibraryError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.is_file() {
        return Err(BlueprintLibraryError::NotAFile {
            path: path.to_path_buf(),
        });
    }
    if metadata.len() > MAX_PACKAGE_BYTES {
        return Err(BlueprintLibraryError::TooLarge {
            path: path.to_path_buf(),
            bytes: metadata.len(),
            limit: MAX_PACKAGE_BYTES,
        });
    }
    let file = File::open(path).map_err(|source| BlueprintLibraryError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(MAX_PACKAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| BlueprintLibraryError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PACKAGE_BYTES {
        return Err(BlueprintLibraryError::TooLarge {
            path: path.to_path_buf(),
            bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            limit: MAX_PACKAGE_BYTES,
        });
    }
    Ok(bytes)
}

fn existing_entry(
    target: &Path,
    metadata: &fs::Metadata,
    hash: &str,
    bytes: &[u8],
) -> Result<LibraryInsert, BlueprintLibraryError> {
    if metadata.file_type().is_symlink() {
        return Err(BlueprintLibraryError::PathEscape {
            path: target.to_path_buf(),
        });
    }
    if !metadata.is_file() {
        return Err(BlueprintLibraryError::Collision {
            hash: hash.to_string(),
            path: target.to_path_buf(),
        });
    }
    let existing = read_stored_file(target, hash)?;
    if existing == bytes {
        return Ok(LibraryInsert {
            hash: hash.to_string(),
            path: target.to_path_buf(),
            inserted: false,
        });
    }
    Err(BlueprintLibraryError::Collision {
        hash: hash.to_string(),
        path: target.to_path_buf(),
    })
}

fn read_stored_file(path: &Path, hash: &str) -> Result<Vec<u8>, BlueprintLibraryError> {
    let file = File::open(path).map_err(|source| BlueprintLibraryError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_PACKAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| BlueprintLibraryError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PACKAGE_BYTES {
        return Err(BlueprintLibraryError::Collision {
            hash: hash.to_string(),
            path: path.to_path_buf(),
        });
    }
    Ok(bytes)
}

fn write_temp_package(shard: &Path, bytes: &[u8]) -> Result<PathBuf, BlueprintLibraryError> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let name = format!(".insert-{}-{stamp}-{unique}.tmp", std::process::id());
        let path = shard.join(name);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(BlueprintLibraryError::Io { path, source });
            }
        };
        let result = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| BlueprintLibraryError::Io {
                path: path.clone(),
                source,
            });
        drop(file);
        if let Err(error) = result {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        return Ok(path);
    }
    Err(BlueprintLibraryError::Io {
        path: shard.join(".insert.tmp"),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary package name",
        ),
    })
}

fn cleanup_empty_shard(shard: &Path, created: bool) {
    if created {
        let _ = fs::remove_dir(shard);
    }
}
