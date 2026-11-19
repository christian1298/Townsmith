//! Chunk dimensions and material identifiers.

/// Chunk edge length in voxels. A 10 cm voxel makes a chunk 3.2 m per side.
pub const CHUNK_SIZE: usize = 32;

/// Number of voxels in one chunk.
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// Edge length of one voxel in meters.
pub const VOXEL_SIZE_M: f32 = 0.1;

/// Material identifier. `0` is always air.
pub type MaterialId = u16;

/// The empty material.
pub const AIR: MaterialId = 0;

/// Linear index for a local chunk coordinate.
///
/// Layout is `x + y * 32 + z * 32 * 32`.
pub(crate) fn linear_index(x: usize, y: usize, z: usize) -> usize {
    x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE
}

/// Convert a linear index back to local chunk coordinates.
pub(crate) fn coords_from_index(index: usize) -> (usize, usize, usize) {
    let x = index % CHUNK_SIZE;
    let y = (index / CHUNK_SIZE) % CHUNK_SIZE;
    let z = index / (CHUNK_SIZE * CHUNK_SIZE);
    (x, y, z)
}
