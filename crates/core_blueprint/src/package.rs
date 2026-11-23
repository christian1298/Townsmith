//! Atomic `.tsbp` zip save/load.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};

use core_voxel::volume::VoxelVolume;

use crate::error::BlueprintError;
use crate::format::{decode_voxels, encode_voxels};
use crate::hash::compute_hash;
use crate::limits::{MAX_METADATA_BYTES, MAX_SOLID_VOXELS, MAX_VOXEL_STREAM_BYTES};
use crate::metadata::{BlueprintMetadata, FORMAT_VERSION};

const JSON_ENTRY: &str = "blueprint.json";
const VOXEL_ENTRY: &str = "voxels.bin";
const THUMBNAIL_ENTRY: &str = "thumbnail.png";

/// A loaded blueprint: metadata plus editable volume.
#[derive(Debug, Clone, PartialEq)]
pub struct Blueprint {
    /// Package metadata.
    pub metadata: BlueprintMetadata,
    /// Voxel volume.
    pub volume: VoxelVolume,
}

impl Blueprint {
    /// Create a blueprint and synchronize the derived metadata fields.
    pub fn new(mut metadata: BlueprintMetadata, volume: VoxelVolume) -> Self {
        metadata.format_version = FORMAT_VERSION;
        metadata.bounds_m = volume.config().bounds_m();
        metadata.solid_voxels = u64::from(volume.solid_count());
        Self { metadata, volume }
    }
}

/// Save a blueprint atomically.
///
/// The package is written to a temporary file in the destination directory,
/// synced, and renamed. A failed save never leaves a partial target. On
/// Windows an existing destination is removed before the retry, so the final
/// overwrite is not atomic.
pub fn save_tsbp(path: &Path, blueprint: &Blueprint) -> Result<(), BlueprintError> {
    save_tsbp_with_hash(path, blueprint).map(|_| ())
}

/// Save a blueprint atomically and return the content hash written to disk.
///
/// This avoids encoding the voxel stream twice when a caller needs the hash
/// (for example, the editor save result).
pub fn save_tsbp_with_hash(path: &Path, blueprint: &Blueprint) -> Result<String, BlueprintError> {
    let voxels = encode_voxels(&blueprint.volume)?;
    if voxels.len() > MAX_VOXEL_STREAM_BYTES {
        return Err(BlueprintError::TooLarge {
            what: "voxel stream",
            limit: MAX_VOXEL_STREAM_BYTES as u64,
        });
    }

    let mut metadata = blueprint.metadata.clone();
    metadata.format_version = FORMAT_VERSION;
    metadata.bounds_m = blueprint.volume.config().bounds_m();
    metadata.solid_voxels = u64::from(blueprint.volume.solid_count());
    if metadata.solid_voxels > MAX_SOLID_VOXELS {
        return Err(BlueprintError::TooLarge {
            what: "solid voxel count",
            limit: MAX_SOLID_VOXELS,
        });
    }
    validate_metadata(&metadata)?;
    let content_hash = compute_hash(&metadata, &voxels)?;
    metadata.content_hash = content_hash.clone();
    let json = serde_json::to_vec_pretty(&metadata)?;
    if json.len() > MAX_METADATA_BYTES {
        return Err(BlueprintError::TooLarge {
            what: "metadata",
            limit: MAX_METADATA_BYTES as u64,
        });
    }

    let temp = temp_path(path);
    let result = write_package(&temp, &json, &voxels);
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }

    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(content_hash)
}

fn write_package(path: &Path, json: &[u8], voxels: &[u8]) -> Result<(), BlueprintError> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);

    zip.start_file(JSON_ENTRY, options)?;
    zip.write_all(json)?;
    zip.start_file(VOXEL_ENTRY, options)?;
    zip.write_all(voxels)?;
    let file = zip.finish()?;
    file.sync_all()?;
    Ok(())
}

/// Load a `.tsbp` package and verify its hash and metadata.
pub fn load_tsbp(path: &Path) -> Result<Blueprint, BlueprintError> {
    let file = File::open(path)?;
    let archive = zip::ZipArchive::new(file)?;
    load_archive(archive)
}

/// Load and verify a `.tsbp` package from bytes already held in memory.
///
/// This is the same validation as [`load_tsbp`], used by the local blueprint
/// library so the bytes it hashes and stores are exactly the bytes it
/// validated. The caller owns the size cap; this function does not copy the
/// input.
pub(crate) fn load_tsbp_bytes(bytes: &[u8]) -> Result<Blueprint, BlueprintError> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    load_archive(archive)
}

fn load_archive<R: Read + Seek>(
    mut archive: zip::ZipArchive<R>,
) -> Result<Blueprint, BlueprintError> {
    if archive.has_duplicate_names() {
        return Err(BlueprintError::DuplicateEntry(
            "duplicate package entry".to_string(),
        ));
    }

    let mut names = HashSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name()?.into_owned();
        if entry.is_dir() {
            return Err(BlueprintError::UnknownEntry(name));
        }
        if !names.insert(name.clone()) {
            return Err(BlueprintError::DuplicateEntry(name));
        }
        if name != JSON_ENTRY && name != VOXEL_ENTRY && name != THUMBNAIL_ENTRY {
            return Err(BlueprintError::UnknownEntry(name));
        }
    }

    let json = read_entry(&mut archive, JSON_ENTRY, MAX_METADATA_BYTES as u64)?;
    let metadata: BlueprintMetadata = serde_json::from_slice(&json)?;
    if metadata.format_version != FORMAT_VERSION {
        return Err(BlueprintError::UnsupportedVersion(metadata.format_version));
    }
    if metadata.content_hash.is_empty() {
        return Err(BlueprintError::Format(
            "blueprint is missing content_hash".to_string(),
        ));
    }
    validate_metadata(&metadata)?;

    let voxels = read_entry(&mut archive, VOXEL_ENTRY, MAX_VOXEL_STREAM_BYTES as u64)?;
    let decoded = decode_voxels(&voxels)?;
    if decoded.solid_voxels > MAX_SOLID_VOXELS {
        return Err(BlueprintError::TooLarge {
            what: "solid voxel count",
            limit: MAX_SOLID_VOXELS,
        });
    }
    if decoded.solid_voxels != metadata.solid_voxels {
        return Err(BlueprintError::Format(format!(
            "metadata declares {} solid voxels, stream has {}",
            metadata.solid_voxels, decoded.solid_voxels
        )));
    }

    let expected_bounds = decoded.volume.config().bounds_m();
    for (index, (expected, actual)) in expected_bounds
        .iter()
        .zip(metadata.bounds_m.iter())
        .enumerate()
    {
        if (expected - actual).abs() > 0.001 {
            return Err(BlueprintError::Format(format!(
                "metadata bounds axis {index} is {actual}, expected {expected}"
            )));
        }
    }

    let actual_hash = compute_hash(&metadata, &voxels)?;
    if actual_hash != metadata.content_hash {
        return Err(BlueprintError::HashMismatch {
            expected: metadata.content_hash.clone(),
            actual: actual_hash,
        });
    }

    if !metadata.dependencies.mods.is_empty() {
        return Err(BlueprintError::MissingDependency(format!(
            "mods: {}",
            metadata.dependencies.mods.join(", ")
        )));
    }
    if !metadata.dependencies.resource_packs.is_empty() {
        return Err(BlueprintError::MissingDependency(format!(
            "resource packs: {}",
            metadata.dependencies.resource_packs.join(", ")
        )));
    }

    Ok(Blueprint {
        metadata,
        volume: decoded.volume,
    })
}

fn validate_metadata(metadata: &BlueprintMetadata) -> Result<(), BlueprintError> {
    if metadata.id.trim().is_empty() {
        return Err(BlueprintError::Format("blueprint id is empty".to_string()));
    }
    if metadata.name.trim().is_empty() {
        return Err(BlueprintError::Format(
            "blueprint name is empty".to_string(),
        ));
    }
    if metadata.bounds_m.iter().any(|value| !value.is_finite()) {
        return Err(BlueprintError::Format(
            "blueprint bounds contain a non-finite value".to_string(),
        ));
    }
    for room in &metadata.rooms {
        if !room.area_m2.is_finite() || !room.volume_m3.is_finite() {
            return Err(BlueprintError::Format(format!(
                "room {} contains a non-finite area or volume",
                room.id
            )));
        }
    }
    if metadata.props.len() > crate::limits::MAX_PROPS {
        return Err(BlueprintError::TooLarge {
            what: "props",
            limit: crate::limits::MAX_PROPS as u64,
        });
    }
    for prop in &metadata.props {
        if prop.position.iter().any(|value| !value.is_finite()) || !prop.rotation.is_finite() {
            return Err(BlueprintError::Format(format!(
                "prop {} contains a non-finite transform",
                prop.id
            )));
        }
    }
    if !metadata.stats.material_cost.is_finite() || metadata.stats.material_cost < 0.0 {
        return Err(BlueprintError::Format(
            "stats.material_cost must be finite and non-negative".to_string(),
        ));
    }
    if let Some(structure) = &metadata.structure {
        // Drafts may exceed material limits; placement and load revalidate
        // the full structure. Fields and volume height must still be sane.
        structure
            .validate_declared_against_bounds(metadata.bounds_m)
            .map_err(|error| BlueprintError::Format(format!("structure: {error}")))?;
    }
    Ok(())
}

fn read_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &'static str,
    limit: u64,
) -> Result<Vec<u8>, BlueprintError> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => {
            return Err(BlueprintError::MissingEntry(name));
        }
        Err(error) => return Err(error.into()),
    };
    let mut data = Vec::new();
    entry.by_ref().take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(BlueprintError::TooLarge { what: name, limit });
    }
    Ok(data)
}

fn temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("blueprint.tsbp");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    parent.join(format!(".{name}.{}-{stamp}.tmp", std::process::id()))
}
