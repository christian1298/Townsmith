//! Deterministic terrain worldgen, edit deltas, and chunk streaming.
//!
//! The world uses a 1 m terrain grid. A world chunk is 256 x 256 cells
//! (65,536 cells) and is generated deterministically from a seed. Player
//! edits are sparse deltas applied over the procedural base.

pub mod biomes;
pub mod chunk;
pub mod coords;
pub mod delta;
pub mod error;
pub mod generator;
pub mod mesh;
pub mod noise;
pub mod streaming;

pub use biomes::Biome;
pub use chunk::WorldChunk;
pub use coords::{
    TerrainPos, WorldChunkCoord, MAX_HEIGHT_M, WATER_LEVEL_M, WORLD_CHUNK_CELLS,
    WORLD_CHUNK_METERS, WORLD_CHUNK_SIZE,
};
pub use delta::{DeltaStore, TerrainEdit};
pub use error::WorldError;
pub use generator::WorldGenerator;
pub use mesh::{terrain_mesh, TerrainMesh, TerrainVertex};
pub use streaming::{StreamUpdate, StreamingConfig, StreamingWorld};
