//! A 32 x 32 x 32 voxel chunk.

use crate::coords::{linear_index, MaterialId, AIR, CHUNK_SIZE, CHUNK_VOLUME};
use crate::error::VoxelError;
use crate::palette::EncodedChunk;

/// A dense in-memory chunk.
///
/// The dense array gives O(1) reads and writes. Persistence and network
/// transfer use [`EncodedChunk`], which stores a palette and RLE runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoxelChunk {
    voxels: Vec<MaterialId>,
    solid_count: u32,
}

impl Default for VoxelChunk {
    fn default() -> Self {
        Self::new()
    }
}

impl VoxelChunk {
    /// Create an all-air chunk.
    #[must_use]
    pub fn new() -> Self {
        Self {
            voxels: vec![AIR; CHUNK_VOLUME],
            solid_count: 0,
        }
    }

    /// Build a chunk from a dense array of exactly [`CHUNK_VOLUME`] voxels.
    pub fn from_voxels(voxels: Vec<MaterialId>) -> Result<Self, VoxelError> {
        if voxels.len() != CHUNK_VOLUME {
            return Err(VoxelError::InvalidRunLength {
                expected: CHUNK_VOLUME,
                actual: voxels.len(),
            });
        }
        let solid_count = voxels.iter().filter(|&&m| m != AIR).count() as u32;
        Ok(Self {
            voxels,
            solid_count,
        })
    }

    /// Read a voxel. Out-of-bounds coordinates read as air.
    #[must_use]
    pub fn get(&self, x: usize, y: usize, z: usize) -> MaterialId {
        self.index(x, y, z).map_or(AIR, |index| self.voxels[index])
    }

    /// Read a voxel, returning `None` when the coordinate is outside the chunk.
    #[must_use]
    pub fn get_checked(&self, x: usize, y: usize, z: usize) -> Option<MaterialId> {
        self.index(x, y, z).map(|index| self.voxels[index])
    }

    /// Write a voxel.
    pub fn set(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        material: MaterialId,
    ) -> Result<(), VoxelError> {
        let index = self
            .index(x, y, z)
            .ok_or(VoxelError::OutOfBounds { x, y, z })?;
        let previous = self.voxels[index];
        if previous == material {
            return Ok(());
        }

        match (previous == AIR, material == AIR) {
            (true, false) => self.solid_count += 1,
            (false, true) => self.solid_count -= 1,
            _ => {}
        }
        self.voxels[index] = material;
        Ok(())
    }

    /// Fill a half-open box `[min, max)` with a material.
    ///
    /// Bounds are validated before any voxel is changed.
    pub fn fill_box(
        &mut self,
        min: [usize; 3],
        max: [usize; 3],
        material: MaterialId,
    ) -> Result<(), VoxelError> {
        for (axis, (&lo, &hi)) in min.iter().zip(max.iter()).enumerate() {
            if lo > hi || hi > CHUNK_SIZE {
                return Err(VoxelError::OutOfBounds {
                    x: if axis == 0 { hi } else { min[0] },
                    y: if axis == 1 { hi } else { min[1] },
                    z: if axis == 2 { hi } else { min[2] },
                });
            }
        }

        for z in min[2]..max[2] {
            for y in min[1]..max[1] {
                for x in min[0]..max[0] {
                    self.set(x, y, z, material)?;
                }
            }
        }
        Ok(())
    }

    /// Remove every voxel.
    pub fn clear(&mut self) {
        self.voxels.fill(AIR);
        self.solid_count = 0;
    }

    /// Number of non-air voxels.
    #[must_use]
    pub fn solid_count(&self) -> u32 {
        self.solid_count
    }

    /// Whether the chunk contains only air.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.solid_count == 0
    }

    /// Read-only dense voxel data.
    #[must_use]
    pub fn as_slice(&self) -> &[MaterialId] {
        &self.voxels
    }

    /// Encode to palette + RLE.
    #[must_use]
    pub fn encode(&self) -> EncodedChunk {
        EncodedChunk::from_chunk(self)
    }

    /// Decode a palette + RLE chunk.
    pub fn decode(encoded: &EncodedChunk) -> Result<Self, VoxelError> {
        encoded.decode()
    }

    fn index(&self, x: usize, y: usize, z: usize) -> Option<usize> {
        if x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE {
            Some(linear_index(x, y, z))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_chunk_is_air() {
        let chunk = VoxelChunk::new();
        assert!(chunk.is_empty());
        assert_eq!(chunk.solid_count(), 0);
        assert_eq!(chunk.get(0, 0, 0), AIR);
        assert_eq!(chunk.get(31, 31, 31), AIR);
    }

    #[test]
    fn set_and_get_roundtrip() {
        let mut chunk = VoxelChunk::new();
        chunk.set(3, 4, 5, 7).unwrap();
        assert_eq!(chunk.get(3, 4, 5), 7);
        assert_eq!(chunk.get(3, 4, 4), AIR);
        assert_eq!(chunk.solid_count(), 1);
    }

    #[test]
    fn out_of_bounds_is_rejected_for_writes() {
        let mut chunk = VoxelChunk::new();
        assert_eq!(
            chunk.set(32, 0, 0, 1),
            Err(VoxelError::OutOfBounds { x: 32, y: 0, z: 0 })
        );
        assert_eq!(chunk.get(32, 0, 0), AIR);
        assert_eq!(chunk.get_checked(32, 0, 0), None);
    }

    #[test]
    fn solid_count_tracks_air_transitions() {
        let mut chunk = VoxelChunk::new();
        chunk.set(1, 1, 1, 2).unwrap();
        chunk.set(1, 1, 1, 3).unwrap();
        assert_eq!(chunk.solid_count(), 1);
        chunk.set(1, 1, 1, AIR).unwrap();
        assert_eq!(chunk.solid_count(), 0);
    }

    #[test]
    fn fill_box_fills_and_clears() {
        let mut chunk = VoxelChunk::new();
        chunk.fill_box([1, 2, 3], [4, 5, 6], 9).unwrap();
        assert_eq!(chunk.solid_count(), 27);
        assert_eq!(chunk.get(1, 2, 3), 9);
        assert_eq!(chunk.get(3, 4, 5), 9);
        assert_eq!(chunk.get(4, 5, 6), AIR);

        chunk.fill_box([1, 2, 3], [4, 5, 6], AIR).unwrap();
        assert_eq!(chunk.solid_count(), 0);
    }

    #[test]
    fn fill_box_rejects_invalid_bounds_without_partial_write() {
        let mut chunk = VoxelChunk::new();
        assert!(chunk
            .fill_box([0, 0, 0], [CHUNK_SIZE + 1, 1, 1], 1)
            .is_err());
        assert_eq!(chunk.solid_count(), 0);
    }

    #[test]
    fn encode_decode_roundtrip() {
        let mut chunk = VoxelChunk::new();
        chunk.fill_box([0, 0, 0], [8, 4, 8], 1).unwrap();
        chunk.fill_box([8, 0, 0], [16, 4, 8], 2).unwrap();
        chunk.set(0, 10, 0, 3).unwrap();

        let encoded = chunk.encode();
        let decoded = VoxelChunk::decode(&encoded).unwrap();
        assert_eq!(decoded, chunk);
    }
}
