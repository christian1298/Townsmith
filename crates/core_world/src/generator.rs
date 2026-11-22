//! Deterministic terrain generation.
//!
//! World positions are sampled through `f32` because the noise field is
//! `f32`-based. IEEE-754 binary32 represents every integer exactly only up to
//! `2^24 = 16,777,216`; above that the spacing is at least 2 m, so adjacent
//! 1 m cells can quantize to the same sample and distant terrain appears
//! flat-stepped. That is the supported world-coordinate bound: `|x|, |z| <=
//! 2^24 m`. Chunk and save coordinates stay `i64`, and rendering rebases with a
//! floating origin, so the bound applies to sampling only. Moving to `f64`
//! sampling would change every deterministic height (and the pinned worldgen /
//! streaming checksums) and therefore needs a `WORLDGEN_VERSION` bump and a
//! deliberate golden update, not a silent migration.

use crate::biomes::Biome;
use crate::chunk::WorldChunk;
use crate::coords::{TerrainPos, WorldChunkCoord, MAX_HEIGHT_M, WATER_LEVEL_M, WORLD_CHUNK_SIZE};
use crate::error::WorldError;
use crate::noise::fbm2d;

const CONTINENT_SEED: u64 = 0xA076_1D64_78BD_642F;
const ELEVATION_SEED: u64 = 0xE703_7ED1_A0B4_28DB;
const DETAIL_SEED: u64 = 0x8EBC_6AF0_9C88_C6E3;
const RIVER_SEED: u64 = 0x5899_65CC_7537_4CC3;
const MOISTURE_SEED: u64 = 0x1D8E_4E27_C47D_124F;

/// Version of the worldgen algorithm stored in saves.
pub const WORLDGEN_VERSION: u16 = 1;

/// Deterministic terrain generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldGenerator {
    seed: u64,
}

impl WorldGenerator {
    /// Create a generator for a world seed.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// The world seed.
    #[must_use]
    pub const fn seed(self) -> u64 {
        self.seed
    }

    /// Generate one 256 x 256 terrain chunk.
    pub fn generate(&self, coord: WorldChunkCoord) -> Result<WorldChunk, WorldError> {
        let origin = coord.origin_m();
        let mut heights = Vec::with_capacity(crate::coords::WORLD_CHUNK_CELLS);
        let mut biomes = Vec::with_capacity(crate::coords::WORLD_CHUNK_CELLS);

        for local_z in 0..WORLD_CHUNK_SIZE {
            for local_x in 0..WORLD_CHUNK_SIZE {
                let wx = (origin.x + local_x as i64) as f32;
                let wz = (origin.z + local_z as i64) as f32;
                let (height, biome) = self.sample(wx, wz);
                heights.push(height);
                biomes.push(biome);
            }
        }

        WorldChunk::from_parts(coord, heights, biomes)
    }

    /// Sample one terrain cell. Exposed for seam tests and tools.
    #[must_use]
    pub fn sample(&self, wx: f32, wz: f32) -> (u16, Biome) {
        let continent = fbm2d(
            wx * 0.0012,
            wz * 0.0012,
            self.seed ^ CONTINENT_SEED,
            4,
            2.0,
            0.5,
        );
        let elevation = fbm2d(
            wx * 0.009,
            wz * 0.009,
            self.seed ^ ELEVATION_SEED,
            5,
            2.0,
            0.5,
        );
        let detail = fbm2d(wx * 0.06, wz * 0.06, self.seed ^ DETAIL_SEED, 3, 2.0, 0.5);
        let river = fbm2d(
            wx * 0.0025,
            wz * 0.0025,
            self.seed ^ RIVER_SEED,
            3,
            2.0,
            0.5,
        );
        let moisture = fbm2d(
            wx * 0.004,
            wz * 0.004,
            self.seed ^ MOISTURE_SEED,
            3,
            2.0,
            0.5,
        );

        let mut height = 30.0 + continent * 18.0 + elevation * 12.0 + detail * 3.0;
        let river_valley = river.abs() < 0.035;
        if river_valley {
            height = height.min(f32::from(WATER_LEVEL_M) - 1.0);
        }
        let height = height.clamp(1.0, f32::from(MAX_HEIGHT_M)).round() as u16;

        let biome = if river_valley {
            Biome::RiverValley
        } else if height <= WATER_LEVEL_M + 1 {
            Biome::Coast
        } else if height > 34 {
            Biome::Hills
        } else if moisture > 0.10 {
            Biome::Forest
        } else {
            Biome::Plains
        };

        (height, biome)
    }

    /// Height at a world position, without building a full chunk.
    #[must_use]
    pub fn height_at(&self, pos: TerrainPos) -> u16 {
        self.sample(pos.x as f32, pos.z as f32).0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::WORLD_CHUNK_CELLS;

    #[test]
    fn generation_is_deterministic() {
        let generator = WorldGenerator::new(12345);
        let a = generator
            .generate(WorldChunkCoord::new(0, 0))
            .expect("chunk");
        let b = generator
            .generate(WorldChunkCoord::new(0, 0))
            .expect("chunk");
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_produce_different_terrain() {
        let a = WorldGenerator::new(1)
            .generate(WorldChunkCoord::new(0, 0))
            .expect("chunk");
        let b = WorldGenerator::new(2)
            .generate(WorldChunkCoord::new(0, 0))
            .expect("chunk");
        assert_ne!(a.heights(), b.heights());
    }

    #[test]
    fn heights_are_within_range() {
        let chunk = WorldGenerator::new(7)
            .generate(WorldChunkCoord::new(-2, 3))
            .expect("chunk");
        assert!(chunk
            .heights()
            .iter()
            .all(|&h| (1..=MAX_HEIGHT_M).contains(&h)));
        assert_eq!(chunk.heights().len(), WORLD_CHUNK_CELLS);
        assert_eq!(chunk.biomes().len(), WORLD_CHUNK_CELLS);
    }

    #[test]
    fn chunk_edges_sample_global_coordinates() {
        let generator = WorldGenerator::new(99);
        let left = generator
            .generate(WorldChunkCoord::new(0, 0))
            .expect("chunk");
        let right = generator
            .generate(WorldChunkCoord::new(1, 0))
            .expect("chunk");
        for z in 0..WORLD_CHUNK_SIZE {
            let left_pos = TerrainPos::new(255, z as i64);
            let right_pos = TerrainPos::new(256, z as i64);
            assert_eq!(left.height(255, z), Some(generator.height_at(left_pos)));
            assert_eq!(right.height(0, z), Some(generator.height_at(right_pos)));
        }
    }

    #[test]
    fn chunk_edges_sample_global_coordinates_across_negative_chunks() {
        let generator = WorldGenerator::new(99);
        let left = generator
            .generate(WorldChunkCoord::new(-1, 0))
            .expect("chunk");
        let right = generator
            .generate(WorldChunkCoord::new(0, 0))
            .expect("chunk");
        for z in 0..WORLD_CHUNK_SIZE {
            let left_pos = TerrainPos::new(-1, z as i64);
            let right_pos = TerrainPos::new(0, z as i64);
            assert_eq!(left.height(255, z), Some(generator.height_at(left_pos)));
            assert_eq!(right.height(0, z), Some(generator.height_at(right_pos)));
        }
    }

    #[test]
    fn height_at_matches_generated_chunk() {
        let generator = WorldGenerator::new(2026);
        let chunk = generator
            .generate(WorldChunkCoord::new(2, -3))
            .expect("chunk");
        let pos = TerrainPos::new(2 * 256 + 17, -3 * 256 + 23);
        assert_eq!(chunk.height_at_world(pos), Some(generator.height_at(pos)));
    }
}
