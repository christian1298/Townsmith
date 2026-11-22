#![deny(unsafe_op_in_unsafe_fn)]
//! Engine-agnostic voxel primitives for Townsmith.
//!
//! A chunk is a 32 x 32 x 32 grid of 10 cm voxels (3.2 m per side).
//! In memory the grid is flat for O(1) access; persistence uses a palette
//! plus run-length encoding. A [`VoxelVolume`] groups chunks into a larger
//! editable box and meshes it as one buffer. The greedy mesher emits
//! engine-neutral mesh buffers that the shell can upload to any renderer.

pub mod chunk;
pub mod coords;
pub mod demo;
pub mod error;
pub mod mesh;
pub mod palette;
pub mod volume;

pub use chunk::VoxelChunk;
pub use coords::{MaterialId, AIR, CHUNK_SIZE, CHUNK_VOLUME, VOXEL_SIZE_M};
pub use error::VoxelError;
pub use mesh::{greedy_mesh, greedy_mesh_with_sampler, MeshBuffers, MeshVertex};
pub use palette::{EncodedChunk, RleRun, MAX_PALETTE_ENTRIES};
pub use volume::{
    ChunkPos, VolumeConfig, VolumeConfigError, VolumePos, VoxelVolume, MAX_CHUNKS,
    MAX_CHUNKS_PER_AXIS,
};
