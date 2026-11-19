//! Palette and run-length encoding for chunks.

use crate::chunk::VoxelChunk;
use crate::coords::{coords_from_index, MaterialId, AIR, CHUNK_VOLUME};
use crate::error::VoxelError;

/// One run of identical palette entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RleRun {
    /// Index into [`EncodedChunk::palette`].
    pub palette_index: u16,
    /// Number of voxels in the run.
    pub count: u16,
}

/// A palette plus run-length encoded chunk.
///
/// Palette entry 0 is always air. Every material used by the chunk appears
/// exactly once in the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedChunk {
    /// Material palette. Entry 0 is air.
    pub palette: Vec<MaterialId>,
    /// Runs in linear chunk order.
    pub runs: Vec<RleRun>,
}

impl EncodedChunk {
    /// Encode a dense chunk.
    #[must_use]
    pub fn from_chunk(chunk: &VoxelChunk) -> Self {
        let mut palette = vec![AIR];
        for &material in chunk.as_slice() {
            if !palette.contains(&material) {
                palette.push(material);
            }
        }

        let mut runs = Vec::new();
        let mut current_index = 0u16;
        let mut count = 0u16;
        for &material in chunk.as_slice() {
            let index = palette
                .iter()
                .position(|&entry| entry == material)
                .expect("palette contains every material in the chunk")
                as u16;

            if count == 0 {
                current_index = index;
                count = 1;
            } else if index == current_index {
                count += 1;
            } else {
                runs.push(RleRun {
                    palette_index: current_index,
                    count,
                });
                current_index = index;
                count = 1;
            }
        }

        if count > 0 {
            runs.push(RleRun {
                palette_index: current_index,
                count,
            });
        }

        Self { palette, runs }
    }

    /// Decode into a dense chunk.
    pub fn decode(&self) -> Result<VoxelChunk, VoxelError> {
        if self.palette.is_empty() {
            return Err(VoxelError::EmptyPalette);
        }
        if self.palette[0] != AIR {
            return Err(VoxelError::PaletteMustStartWithAir);
        }

        let mut voxels = vec![AIR; CHUNK_VOLUME];
        let mut cursor = 0usize;
        for run in &self.runs {
            if run.count == 0 {
                return Err(VoxelError::ZeroLengthRun);
            }
            let material = *self.palette.get(run.palette_index as usize).ok_or(
                VoxelError::InvalidPaletteIndex {
                    index: run.palette_index,
                    palette_len: self.palette.len(),
                },
            )?;
            let end = cursor + usize::from(run.count);
            if end > CHUNK_VOLUME {
                return Err(VoxelError::InvalidRunLength {
                    expected: CHUNK_VOLUME,
                    actual: end,
                });
            }
            voxels[cursor..end].fill(material);
            cursor = end;
        }

        if cursor != CHUNK_VOLUME {
            return Err(VoxelError::InvalidRunLength {
                expected: CHUNK_VOLUME,
                actual: cursor,
            });
        }

        VoxelChunk::from_voxels(voxels)
    }

    /// Total number of voxels covered by the runs.
    #[must_use]
    pub fn total_voxels(&self) -> usize {
        self.runs.iter().map(|run| usize::from(run.count)).sum()
    }

    /// Decode a single voxel by linear index. Intended for tests and diagnostics.
    #[must_use]
    pub fn voxel_at(&self, index: usize) -> Option<MaterialId> {
        if index >= CHUNK_VOLUME {
            return None;
        }
        let mut cursor = 0usize;
        for run in &self.runs {
            let end = cursor + usize::from(run.count);
            if index < end {
                return self.palette.get(run.palette_index as usize).copied();
            }
            cursor = end;
        }
        None
    }

    /// The local coordinate of a linear voxel index.
    #[must_use]
    pub fn coords_for(index: usize) -> Option<(usize, usize, usize)> {
        if index < CHUNK_VOLUME {
            Some(coords_from_index(index))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::CHUNK_SIZE;

    #[test]
    fn empty_chunk_is_one_air_run() {
        let encoded = VoxelChunk::new().encode();
        assert_eq!(encoded.palette, vec![AIR]);
        assert_eq!(
            encoded.runs,
            vec![RleRun {
                palette_index: 0,
                count: CHUNK_VOLUME as u16,
            }]
        );
    }

    #[test]
    fn uniform_solid_chunk_has_two_palette_entries() {
        let mut chunk = VoxelChunk::new();
        chunk
            .fill_box([0, 0, 0], [CHUNK_SIZE, CHUNK_SIZE, CHUNK_SIZE], 5)
            .unwrap();
        let encoded = chunk.encode();
        assert_eq!(encoded.palette, vec![AIR, 5]);
        assert_eq!(encoded.runs.len(), 1);
        assert_eq!(encoded.runs[0].count, CHUNK_VOLUME as u16);
    }

    #[test]
    fn palette_is_deduplicated() {
        let mut chunk = VoxelChunk::new();
        chunk.set(0, 0, 0, 7).unwrap();
        chunk.set(1, 0, 0, 8).unwrap();
        chunk.set(2, 0, 0, 7).unwrap();
        let encoded = chunk.encode();
        assert_eq!(encoded.palette, vec![AIR, 7, 8]);
    }

    #[test]
    fn decode_rejects_short_stream() {
        let encoded = EncodedChunk {
            palette: vec![AIR, 1],
            runs: vec![RleRun {
                palette_index: 1,
                count: 10,
            }],
        };
        assert!(matches!(
            encoded.decode(),
            Err(VoxelError::InvalidRunLength { .. })
        ));
    }

    #[test]
    fn decode_rejects_bad_palette_index() {
        let encoded = EncodedChunk {
            palette: vec![AIR],
            runs: vec![RleRun {
                palette_index: 1,
                count: CHUNK_VOLUME as u16,
            }],
        };
        assert!(matches!(
            encoded.decode(),
            Err(VoxelError::InvalidPaletteIndex { .. })
        ));
    }

    #[test]
    fn decode_rejects_palette_without_air_first() {
        let encoded = EncodedChunk {
            palette: vec![1],
            runs: vec![RleRun {
                palette_index: 0,
                count: CHUNK_VOLUME as u16,
            }],
        };
        assert_eq!(encoded.decode(), Err(VoxelError::PaletteMustStartWithAir));
    }

    #[test]
    fn decode_rejects_zero_length_run() {
        let encoded = EncodedChunk {
            palette: vec![AIR],
            runs: vec![RleRun {
                palette_index: 0,
                count: 0,
            }],
        };
        assert_eq!(encoded.decode(), Err(VoxelError::ZeroLengthRun));
    }

    #[test]
    fn voxel_at_reads_encoded_stream() {
        let mut chunk = VoxelChunk::new();
        chunk.set(1, 0, 0, 4).unwrap();
        let encoded = chunk.encode();
        assert_eq!(encoded.voxel_at(0), Some(AIR));
        assert_eq!(encoded.voxel_at(1), Some(4));
        assert_eq!(encoded.voxel_at(CHUNK_VOLUME), None);
    }
}
