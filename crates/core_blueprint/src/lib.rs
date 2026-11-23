//! Versioned `.tsbp` blueprint packages.
//!
//! A package is a zip containing `blueprint.json` and a canonical
//! `voxels.bin` stream. The voxel stream is palette + RLE per non-empty 32³
//! chunk, stored with the zip `Stored` method for v1; zstd/deflate can be
//! added as a new format version.
//!
//! The content hash is BLAKE3 over a domain tag, canonical metadata JSON, and
//! the voxel stream. Loading verifies the hash, chunk coordinates, palette
//! runs, bounds, and solid count.

#![forbid(unsafe_code)]

pub mod error;
pub mod format;
pub mod hash;
pub mod library;
pub mod limits;
pub mod metadata;
pub mod package;

pub use core_structure::{StructuralSystem, StructureError, StructureSpec, StructureSummary};
pub use error::BlueprintError;
pub use format::{decode_voxels, encode_voxels, DecodedVoxels};
pub use hash::compute_hash;
pub use library::{
    BlueprintLibrary, BlueprintLibraryError, LibraryEntry, LibraryInsert, LibrarySummary,
};
pub use limits::{
    MAX_CHUNKS, MAX_METADATA_BYTES, MAX_PACKAGE_BYTES, MAX_SOLID_VOXELS, MAX_VOLUME_VOXELS,
    MAX_VOXEL_STREAM_BYTES,
};
pub use metadata::{
    BlueprintMetadata, BlueprintStats, Dependencies, PropRecord, RoomRecord, FORMAT_VERSION,
};
pub use package::{load_tsbp, save_tsbp, save_tsbp_with_hash, Blueprint};
