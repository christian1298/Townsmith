//! Deterministic demo content used by the Phase 0 bridge and benchmarks.

use crate::chunk::VoxelChunk;
use crate::coords::MaterialId;
use crate::error::VoxelError;

/// Wood floor material.
pub const WOOD: MaterialId = 1;
/// Stone wall material.
pub const STONE: MaterialId = 2;
/// Roof material.
pub const ROOF: MaterialId = 3;

/// Build a small hut inside one 32 x 32 x 32 chunk.
///
/// The hut is intentionally simple: a floor, four walls, a door, two windows,
/// a roof, and a chimney. It exercises exposed faces, merged coplanar quads,
/// and multiple materials without needing content assets.
pub fn hut() -> Result<VoxelChunk, VoxelError> {
    let mut chunk = VoxelChunk::new();

    // Floor (one voxel thick).
    chunk.fill_box([4, 0, 4], [28, 1, 28], WOOD)?;

    // Solid shell.
    chunk.fill_box([4, 1, 4], [28, 20, 28], STONE)?;

    // Interior.
    chunk.fill_box([5, 1, 5], [27, 19, 27], crate::AIR)?;

    // Roof and chimney.
    chunk.fill_box([4, 20, 4], [28, 22, 28], ROOF)?;
    chunk.fill_box([22, 20, 22], [25, 25, 25], STONE)?;

    // Door on the south wall (z = 4).
    chunk.fill_box([14, 1, 4], [18, 7, 5], crate::AIR)?;

    // Windows on the west and north walls.
    chunk.fill_box([4, 7, 10], [5, 11, 14], crate::AIR)?;
    chunk.fill_box([10, 7, 27], [14, 11, 28], crate::AIR)?;

    Ok(chunk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::greedy_mesh;

    #[test]
    fn hut_is_not_empty() {
        let chunk = hut().unwrap();
        assert!(chunk.solid_count() > 0);
        assert_eq!(chunk.get(4, 0, 4), WOOD);
        assert_eq!(chunk.get(4, 10, 4), STONE);
        assert_eq!(chunk.get(4, 21, 4), ROOF);
    }

    #[test]
    fn hut_has_a_door_and_windows() {
        let chunk = hut().unwrap();
        assert_eq!(chunk.get(16, 1, 4), crate::AIR);
        assert_eq!(chunk.get(16, 6, 4), crate::AIR);
        assert_eq!(chunk.get(16, 7, 4), STONE);
        assert_eq!(chunk.get(4, 8, 12), crate::AIR);
        assert_eq!(chunk.get(12, 8, 27), crate::AIR);
    }

    #[test]
    fn hut_mesh_has_geometry() {
        let chunk = hut().unwrap();
        let mesh = greedy_mesh(&chunk);
        assert!(mesh.quad_count() > 0);
        assert!(mesh.vertices.len() >= mesh.quad_count() * 4);
    }
}
