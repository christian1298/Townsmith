//! Blueprint package errors.

use std::fmt;
use std::io;

use core_voxel::VoxelError;

/// Errors returned while saving or loading a `.tsbp` package.
#[derive(Debug)]
pub enum BlueprintError {
    /// Filesystem error.
    Io(io::Error),
    /// ZIP container error.
    Zip(zip::result::ZipError),
    /// JSON metadata error.
    Json(serde_json::Error),
    /// Voxel stream error.
    Voxel(VoxelError),
    /// The package is structurally invalid.
    Format(String),
    /// A required entry is missing.
    MissingEntry(&'static str),
    /// A duplicate entry was found.
    DuplicateEntry(String),
    /// An entry outside the package whitelist was found.
    UnknownEntry(String),
    /// The format or game version is unsupported.
    UnsupportedVersion(u16),
    /// The content hash does not match the payload.
    HashMismatch { expected: String, actual: String },
    /// A declared dependency is not available.
    MissingDependency(String),
    /// A size limit was exceeded.
    TooLarge { what: &'static str, limit: u64 },
}

impl fmt::Display for BlueprintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "blueprint I/O error: {error}"),
            Self::Zip(error) => write!(f, "blueprint zip error: {error}"),
            Self::Json(error) => write!(f, "blueprint metadata error: {error}"),
            Self::Voxel(error) => write!(f, "blueprint voxel error: {error}"),
            Self::Format(message) => write!(f, "invalid blueprint: {message}"),
            Self::MissingEntry(name) => write!(f, "blueprint is missing {name}"),
            Self::DuplicateEntry(name) => write!(f, "blueprint contains duplicate {name}"),
            Self::UnknownEntry(name) => write!(f, "blueprint contains unknown entry {name}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported blueprint format version {version}")
            }
            Self::HashMismatch { expected, actual } => write!(
                f,
                "blueprint content hash mismatch: expected {expected}, got {actual}"
            ),
            Self::MissingDependency(dependency) => {
                write!(f, "blueprint requires missing dependency {dependency}")
            }
            Self::TooLarge { what, limit } => {
                write!(f, "blueprint {what} exceeds the {limit}-byte limit")
            }
        }
    }
}

impl std::error::Error for BlueprintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Zip(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Voxel(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for BlueprintError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<zip::result::ZipError> for BlueprintError {
    fn from(error: zip::result::ZipError) -> Self {
        Self::Zip(error)
    }
}

impl From<serde_json::Error> for BlueprintError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<VoxelError> for BlueprintError {
    fn from(error: VoxelError) -> Self {
        Self::Voxel(error)
    }
}
