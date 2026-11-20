//! Canonical content hashing.

use crate::error::BlueprintError;
use crate::metadata::BlueprintMetadata;

const DOMAIN: &[u8] = b"townsmith.tsbp.content.v1\0";

/// Compute the canonical `blake3:<hex>` hash over metadata and voxel bytes.
pub fn compute_hash(metadata: &BlueprintMetadata, voxels: &[u8]) -> Result<String, BlueprintError> {
    let mut canonical = metadata.clone();
    canonical.content_hash.clear();
    let json = serde_json::to_vec(&canonical)?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    hasher.update(&(json.len() as u64).to_le_bytes());
    hasher.update(&json);
    hasher.update(&(voxels.len() as u64).to_le_bytes());
    hasher.update(voxels);
    Ok(format!("blake3:{}", hasher.finalize().to_hex()))
}
