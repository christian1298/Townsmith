//! A 256 x 256 terrain chunk at 1 m resolution.

use crate::biomes::Biome;
use crate::coords::{TerrainPos, WorldChunkCoord, WORLD_CHUNK_CELLS, WORLD_CHUNK_SIZE};
use crate::error::WorldError;

/// A dense 256 x 256 terrain chunk.
///
/// Heights are stored in meters. Biomes are stored as one byte per cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldChunk {
    coord: WorldChunkCoord,
    heights: Vec<u16>,
    biomes: Vec<Biome>,
}

impl WorldChunk {
    /// Build a chunk from dense height and biome arrays.
    pub fn from_parts(
        coord: WorldChunkCoord,
        heights: Vec<u16>,
        biomes: Vec<Biome>,
    ) -> Result<Self, WorldError> {
        if heights.len() != WORLD_CHUNK_CELLS {
            return Err(WorldError::InvalidChunkLength {
                expected: WORLD_CHUNK_CELLS,
                actual: heights.len(),
            });
        }
        if biomes.len() != WORLD_CHUNK_CELLS {
            return Err(WorldError::InvalidChunkLength {
                expected: WORLD_CHUNK_CELLS,
                actual: biomes.len(),
            });
        }
        Ok(Self {
            coord,
            heights,
            biomes,
        })
    }

    /// The chunk coordinate.
    #[must_use]
    pub const fn coord(&self) -> WorldChunkCoord {
        self.coord
    }

    fn index(x: usize, z: usize) -> Result<usize, WorldError> {
        if x >= WORLD_CHUNK_SIZE || z >= WORLD_CHUNK_SIZE {
            return Err(WorldError::OutOfBounds { x, z });
        }
        Ok(x + z * WORLD_CHUNK_SIZE)
    }

    /// Read a height by local cell coordinate.
    #[must_use]
    pub fn height(&self, x: usize, z: usize) -> Option<u16> {
        Self::index(x, z)
            .ok()
            .and_then(|index| self.heights.get(index).copied())
    }

    /// Write a height by local cell coordinate.
    pub fn set_height(&mut self, x: usize, z: usize, height: u16) -> Result<(), WorldError> {
        let index = Self::index(x, z)?;
        self.heights[index] = height;
        Ok(())
    }

    /// Read a biome by local cell coordinate.
    #[must_use]
    pub fn biome(&self, x: usize, z: usize) -> Option<Biome> {
        Self::index(x, z)
            .ok()
            .and_then(|index| self.biomes.get(index).copied())
    }

    /// Write a biome by local cell coordinate.
    pub fn set_biome(&mut self, x: usize, z: usize, biome: Biome) -> Result<(), WorldError> {
        let index = Self::index(x, z)?;
        self.biomes[index] = biome;
        Ok(())
    }

    /// Read a height at a world position, if it belongs to this chunk.
    #[must_use]
    pub fn height_at_world(&self, pos: TerrainPos) -> Option<u16> {
        let (x, z) = self.coord.local_from_world(pos)?;
        self.height(x, z)
    }

    /// Read-only heights, row-major with X contiguous.
    #[must_use]
    pub fn heights(&self) -> &[u16] {
        &self.heights
    }

    /// Read-only biomes, row-major with X contiguous.
    #[must_use]
    pub fn biomes(&self) -> &[Biome] {
        &self.biomes
    }

    /// Minimum height in the chunk.
    #[must_use]
    pub fn min_height(&self) -> u16 {
        self.heights.iter().copied().min().unwrap_or(0)
    }

    /// Maximum height in the chunk.
    #[must_use]
    pub fn max_height(&self) -> u16 {
        self.heights.iter().copied().max().unwrap_or(0)
    }

    /// Total bytes of dense cell data.
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.heights.len() * std::mem::size_of::<u16>() + self.biomes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_chunk() -> WorldChunk {
        WorldChunk::from_parts(
            WorldChunkCoord::new(0, 0),
            vec![24; WORLD_CHUNK_CELLS],
            vec![Biome::Plains; WORLD_CHUNK_CELLS],
        )
        .expect("valid chunk")
    }

    #[test]
    fn builds_a_flat_chunk() {
        let chunk = flat_chunk();
        assert_eq!(chunk.height(0, 0), Some(24));
        assert_eq!(chunk.height(255, 255), Some(24));
        assert_eq!(chunk.biome(17, 42), Some(Biome::Plains));
        assert_eq!(chunk.memory_bytes(), WORLD_CHUNK_CELLS * 3);
    }

    #[test]
    fn writes_heights_and_biomes() {
        let mut chunk = flat_chunk();
        chunk.set_height(4, 5, 80).expect("valid cell");
        chunk.set_biome(4, 5, Biome::Hills).expect("valid cell");
        assert_eq!(chunk.height(4, 5), Some(80));
        assert_eq!(chunk.biome(4, 5), Some(Biome::Hills));
        assert_eq!(chunk.min_height(), 24);
        assert_eq!(chunk.max_height(), 80);
    }

    #[test]
    fn rejects_invalid_coordinates_and_lengths() {
        let mut chunk = flat_chunk();
        assert_eq!(chunk.height(256, 0), None);
        assert_eq!(
            chunk.set_height(0, 256, 1),
            Err(WorldError::OutOfBounds { x: 0, z: 256 })
        );
        let bad = WorldChunk::from_parts(
            WorldChunkCoord::new(0, 0),
            vec![0; WORLD_CHUNK_CELLS - 1],
            vec![Biome::Plains; WORLD_CHUNK_CELLS],
        );
        assert!(matches!(bad, Err(WorldError::InvalidChunkLength { .. })));
    }

    #[test]
    fn maps_world_coordinates_for_negative_chunks() {
        let chunk = WorldChunk::from_parts(
            WorldChunkCoord::new(-1, -1),
            vec![30; WORLD_CHUNK_CELLS],
            vec![Biome::Forest; WORLD_CHUNK_CELLS],
        )
        .expect("valid chunk");
        assert_eq!(chunk.height_at_world(TerrainPos::new(-1, -1)), Some(30));
        assert_eq!(chunk.height_at_world(TerrainPos::new(0, 0)), None);
    }
}
