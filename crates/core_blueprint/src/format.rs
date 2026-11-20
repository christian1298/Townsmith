//! Binary voxel stream (`voxels.bin`) v1.

use std::collections::HashSet;

use core_voxel::palette::RleRun;
use core_voxel::volume::{ChunkPos, VolumeConfig, VoxelVolume};
use core_voxel::{MaterialId, VoxelChunk, CHUNK_SIZE, CHUNK_VOLUME};

use crate::error::BlueprintError;
use crate::limits::{MAX_CHUNKS, MAX_VOXEL_STREAM_BYTES};

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
    let config = VolumeConfig::new(chunks_x, chunks_y, chunks_z)
        .map_err(|error| BlueprintError::Format(error.to_string()))?;
    let count = reader.u32()? as usize;
    let declared_solid = reader.u64()?;
    if count > MAX_CHUNKS {
        return Err(BlueprintError::TooLarge {
            what: "chunk count",
            limit: MAX_CHUNKS as u64,
        });
    }

    let mut volume =
        VoxelVolume::new(config).map_err(|error| BlueprintError::Format(error.to_string()))?;
    let mut seen = HashSet::with_capacity(count);
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
            runs.push(RleRun {
                palette_index: reader.u16()?,
                count: reader.u16()?,
            });
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
