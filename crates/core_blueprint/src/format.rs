//! Binary voxel stream (`voxels.bin`) v1.

use std::collections::HashSet;

use core_voxel::palette::RleRun;
use core_voxel::volume::{ChunkPos, VolumeConfig, VoxelVolume};
use core_voxel::{MaterialId, VoxelChunk, AIR, CHUNK_SIZE, CHUNK_VOLUME};

use crate::error::BlueprintError;
use crate::limits::{MAX_CHUNKS, MAX_SOLID_VOXELS, MAX_VOLUME_VOXELS, MAX_VOXEL_STREAM_BYTES};

const MAGIC: &[u8; 4] = b"TSBV";
const STREAM_VERSION: u16 = 1;
const HEADER_BYTES: usize = 32;

/// Voxels decoded from `voxels.bin`.
#[derive(Debug)]
pub struct DecodedVoxels {
    /// Restored volume.
    pub volume: VoxelVolume,
    /// Solid voxel count declared by the stream and verified after decode.
    pub solid_voxels: u64,
}

/// Encode a volume into the canonical `voxels.bin` v1 stream.
pub fn encode_voxels(volume: &VoxelVolume) -> Result<Vec<u8>, BlueprintError> {
    let config = volume.config();
    let mut chunks: Vec<(ChunkPos, &VoxelChunk)> = Vec::new();
    for x in 0..config.chunks_x {
        for y in 0..config.chunks_y {
            for z in 0..config.chunks_z {
                let pos = ChunkPos::new(x, y, z);
                if let Some(chunk) = volume.chunk_at(pos) {
                    if !chunk.is_empty() {
                        chunks.push((pos, chunk));
                    }
                }
            }
        }
    }
    chunks.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));

    let count = u32::try_from(chunks.len())
        .map_err(|_| BlueprintError::Format("too many non-empty chunks".to_string()))?;
    let solid = volume.solid_count() as u64;

    let mut out = Vec::with_capacity(HEADER_BYTES + chunks.len() * 256);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&STREAM_VERSION.to_le_bytes());
    out.extend_from_slice(&(CHUNK_SIZE as u16).to_le_bytes());
    out.extend_from_slice(&config.chunks_x.to_le_bytes());
    out.extend_from_slice(&config.chunks_y.to_le_bytes());
    out.extend_from_slice(&config.chunks_z.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&solid.to_le_bytes());

    for (pos, chunk) in chunks {
        let encoded = chunk.encode();
        let palette_len = u16::try_from(encoded.palette.len()).map_err(|_| {
            BlueprintError::Format("chunk palette exceeds the u16 length field".to_string())
        })?;
        let run_count = u16::try_from(encoded.runs.len()).map_err(|_| {
            BlueprintError::Format("chunk run count exceeds the u16 length field".to_string())
        })?;
        out.extend_from_slice(&pos.x.to_le_bytes());
        out.extend_from_slice(&pos.y.to_le_bytes());
        out.extend_from_slice(&pos.z.to_le_bytes());
        out.extend_from_slice(&palette_len.to_le_bytes());
        out.extend_from_slice(&run_count.to_le_bytes());
        for material in &encoded.palette {
            out.extend_from_slice(&material.to_le_bytes());
        }
        for run in &encoded.runs {
            out.extend_from_slice(&run.palette_index.to_le_bytes());
            out.extend_from_slice(&run.count.to_le_bytes());
        }
    }

    Ok(out)
}

/// Decode a `voxels.bin` v1 stream.
pub fn decode_voxels(bytes: &[u8]) -> Result<DecodedVoxels, BlueprintError> {
    if bytes.len() > MAX_VOXEL_STREAM_BYTES {
        return Err(BlueprintError::TooLarge {
            what: "voxel stream",
            limit: MAX_VOXEL_STREAM_BYTES as u64,
        });
    }
    if bytes.len() < HEADER_BYTES {
        return Err(BlueprintError::Format(
            "voxel stream is truncated".to_string(),
        ));
    }
    if &bytes[0..4] != MAGIC {
        return Err(BlueprintError::Format("bad voxel stream magic".to_string()));
    }

    let mut reader = Reader { bytes, offset: 4 };
    let version = reader.u16()?;
    if version != STREAM_VERSION {
        return Err(BlueprintError::UnsupportedVersion(version));
    }
    let chunk_size = reader.u16()?;
    if usize::from(chunk_size) != CHUNK_SIZE {
        return Err(BlueprintError::Format(format!(
            "voxel stream chunk size {chunk_size}, expected {CHUNK_SIZE}"
        )));
    }
    let chunks_x = reader.u32()?;
    let chunks_y = reader.u32()?;
    let chunks_z = reader.u32()?;
    // Reject an oversized declared grid before `VolumeConfig`/`VoxelVolume`
    // allocate anything. The dense capacity is the worst-case chunk storage.
    let capacity =
        u128::from(chunks_x) * u128::from(chunks_y) * u128::from(chunks_z) * CHUNK_VOLUME as u128;
    if capacity > u128::from(MAX_VOLUME_VOXELS) {
        return Err(BlueprintError::TooLarge {
            what: "declared volume",
            limit: MAX_VOLUME_VOXELS,
        });
    }
    let config = VolumeConfig::new(chunks_x, chunks_y, chunks_z)
        .map_err(|error| BlueprintError::Format(error.to_string()))?;
    let count = reader.u32()? as usize;
    let declared_solid = reader.u64()?;
    if declared_solid > MAX_SOLID_VOXELS {
        return Err(BlueprintError::TooLarge {
            what: "solid voxel count",
            limit: MAX_SOLID_VOXELS,
        });
    }
    if count > MAX_CHUNKS {
        return Err(BlueprintError::TooLarge {
            what: "chunk count",
            limit: MAX_CHUNKS as u64,
        });
    }

    let mut volume =
        VoxelVolume::new(config).map_err(|error| BlueprintError::Format(error.to_string()))?;
    let mut seen = HashSet::with_capacity(count);
    let mut declared_solid_total: u64 = 0;
    for _ in 0..count {
        let pos = ChunkPos::new(reader.u32()?, reader.u32()?, reader.u32()?);
        if pos.x >= config.chunks_x || pos.y >= config.chunks_y || pos.z >= config.chunks_z {
            return Err(BlueprintError::Format(format!(
                "chunk ({}, {}, {}) is outside the volume",
                pos.x, pos.y, pos.z
            )));
        }
        if !seen.insert((pos.x, pos.y, pos.z)) {
            return Err(BlueprintError::Format(format!(
                "duplicate chunk ({}, {}, {})",
                pos.x, pos.y, pos.z
            )));
        }

        let palette_len = usize::from(reader.u16()?);
        let run_count = usize::from(reader.u16()?);
        if palette_len == 0 || palette_len > 65_535 {
            return Err(BlueprintError::Format("invalid palette length".to_string()));
        }
        if run_count == 0 || run_count > CHUNK_VOLUME.min(65_535) {
            return Err(BlueprintError::Format("invalid run count".to_string()));
        }
        let mut palette = Vec::with_capacity(palette_len);
        for _ in 0..palette_len {
            palette.push(reader.u16()? as MaterialId);
        }
        let mut runs = Vec::with_capacity(run_count);
        for _ in 0..run_count {
            let run = RleRun {
                palette_index: reader.u16()?,
                count: reader.u16()?,
            };
            // Track the declared solid budget while reading runs, before the
            // dense chunk is allocated. Palette index validity is still
            // checked by `VoxelChunk::decode`; an invalid index cannot smuggle
            // solids past this check because the chunk is rejected there.
            if palette
                .get(run.palette_index as usize)
                .is_some_and(|material| *material != AIR)
            {
                declared_solid_total = declared_solid_total.saturating_add(u64::from(run.count));
                if declared_solid_total > MAX_SOLID_VOXELS {
                    return Err(BlueprintError::TooLarge {
                        what: "solid voxel count",
                        limit: MAX_SOLID_VOXELS,
                    });
                }
            }
            runs.push(run);
        }
        let encoded = core_voxel::palette::EncodedChunk { palette, runs };
        let chunk = VoxelChunk::decode(&encoded)?;
        if chunk.is_empty() {
            return Err(BlueprintError::Format(
                "voxel stream contains an empty chunk".to_string(),
            ));
        }
        volume.insert_chunk(pos, chunk)?;
    }

    if reader.offset != bytes.len() {
        return Err(BlueprintError::Format(
            "voxel stream has trailing bytes".to_string(),
        ));
    }
    let actual_solid = volume.solid_count() as u64;
    if actual_solid != declared_solid {
        return Err(BlueprintError::Format(format!(
            "voxel stream declares {declared_solid} solid voxels, decoded {actual_solid}"
        )));
    }
    Ok(DecodedVoxels {
        volume,
        solid_voxels: actual_solid,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], BlueprintError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or_else(|| BlueprintError::Format("voxel stream offset overflow".to_string()))?;
        if end > self.bytes.len() {
            return Err(BlueprintError::Format(
                "voxel stream is truncated".to_string(),
            ));
        }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, BlueprintError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, BlueprintError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, BlueprintError> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::MAX_VOLUME_VOXELS;

    fn header(chunks: [u32; 3], count: u32, declared_solid: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_BYTES);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&STREAM_VERSION.to_le_bytes());
        out.extend_from_slice(&(CHUNK_SIZE as u16).to_le_bytes());
        for axis in chunks {
            out.extend_from_slice(&axis.to_le_bytes());
        }
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&declared_solid.to_le_bytes());
        out
    }

    #[test]
    fn oversized_declared_volume_is_rejected_before_allocation() {
        // 32³ chunks × 32³ voxels = 1,073,741,824 voxels: the densest header
        // the axis limits allow. The capacity guard rejects it before
        // `VolumeConfig`/`VoxelVolume` allocate anything, with zero records.
        let bytes = header([32, 32, 32], 0, 0);
        assert_eq!(bytes.len(), HEADER_BYTES);
        match decode_voxels(&bytes) {
            Err(BlueprintError::TooLarge { what, limit }) => {
                assert_eq!(what, "declared volume");
                assert_eq!(limit, MAX_VOLUME_VOXELS);
            }
            other => panic!("expected a declared-volume rejection, got {other:?}"),
        }

        // 16³ is a valid `VolumeConfig` (4096 chunks) but still far over the
        // decode budget, so the guard must fire before the slot table.
        let bytes = header([16, 16, 16], 0, 0);
        assert!(matches!(
            decode_voxels(&bytes),
            Err(BlueprintError::TooLarge {
                what: "declared volume",
                ..
            })
        ));
    }

    #[test]
    fn declared_solid_over_budget_is_rejected_from_the_header() {
        let bytes = header([1, 1, 1], 0, MAX_SOLID_VOXELS + 1);
        match decode_voxels(&bytes) {
            Err(BlueprintError::TooLarge { what, limit }) => {
                assert_eq!(what, "solid voxel count");
                assert_eq!(limit, MAX_SOLID_VOXELS);
            }
            other => panic!("expected a solid-voxel rejection, got {other:?}"),
        }
    }

    #[test]
    fn rle_solid_runs_over_budget_are_rejected_before_chunk_decode() {
        // One chunk whose non-air runs declare more than the solid budget.
        // The runs are individually valid RLE counts but together exceed both
        // a chunk and the package budget; the incremental guard must fire
        // while reading the runs, before the dense chunk is allocated.
        let mut bytes = header([1, 1, 1], 1, 0);
        for _ in 0..3 {
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        bytes.extend_from_slice(&2u16.to_le_bytes());
        let run_count: u16 = 123;
        bytes.extend_from_slice(&run_count.to_le_bytes());
        bytes.extend_from_slice(&AIR.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        for _ in 0..run_count {
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&u16::MAX.to_le_bytes());
        }
        // 123 × 65,535 = 8,060,805 > 8,000,000.
        assert!(u64::from(run_count) * u64::from(u16::MAX) > MAX_SOLID_VOXELS);
        match decode_voxels(&bytes) {
            Err(BlueprintError::TooLarge { what, limit }) => {
                assert_eq!(what, "solid voxel count");
                assert_eq!(limit, MAX_SOLID_VOXELS);
            }
            other => panic!("expected an incremental solid-voxel rejection, got {other:?}"),
        }
    }
}
