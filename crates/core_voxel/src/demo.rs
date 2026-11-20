//! Deterministic demo content used by the Phase 0 bridge and benchmarks.

use crate::chunk::VoxelChunk;
use crate::coords::MaterialId;
use crate::error::VoxelError;
use crate::volume::{VolumeConfig, VolumePos, VoxelVolume};

/// Wood floor material.
pub const WOOD: MaterialId = 1;
/// Stone wall material.
pub const STONE: MaterialId = 2;
/// Roof material.
pub const ROOF: MaterialId = 3;
/// Interior partition and stair material.
pub const PARTITION: MaterialId = 4;

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

/// Build the deterministic two-story vertical-slice house.
///
/// The house fills the default 5 × 3 × 5 chunk volume
/// (16 m × 9.6 m × 16 m): a wood ground slab, a stone perimeter shell, a
/// wood first-floor slab with a stairwell opening, a roof with a 0.2 m
/// overhang, interior partitions, a front door, windows on both floors of all
/// four walls, and a straight stair. It exercises multi-chunk greedy meshing,
/// mixed materials, holes, and interior geometry without content assets.
///
/// The fixture is shared by the `townsmith-bench` volume benchmark and the
/// Godot slice-house benchmark, so keep it deterministic.
pub fn two_story_house() -> Result<VoxelVolume, VoxelError> {
    // The default config is a compile-time-valid 5x3x5 volume; a failure here
    // is a programming error, not a caller-facing condition.
    let mut volume =
        VoxelVolume::new(VolumeConfig::default()).expect("default volume config must be valid");
    build_two_story_house(&mut volume)?;
    Ok(volume)
}

fn build_two_story_house(volume: &mut VoxelVolume) -> Result<(), VoxelError> {
    fn pos(x: u32, y: u32, z: u32) -> VolumePos {
        VolumePos::new(x, y, z)
    }

    // Ground floor slab.
    volume.fill_box(pos(8, 0, 8), pos(152, 2, 152), WOOD)?;

    // Perimeter shell, then hollow the interior.
    volume.fill_box(pos(8, 2, 8), pos(152, 80, 152), STONE)?;
    volume.fill_box(pos(10, 2, 10), pos(150, 80, 150), crate::AIR)?;

    // First-floor slab with a stairwell opening.
    volume.fill_box(pos(10, 40, 10), pos(150, 42, 150), WOOD)?;
    volume.fill_box(pos(112, 40, 20), pos(136, 42, 58), crate::AIR)?;

    // Roof with a 0.2 m overhang.
    volume.fill_box(pos(6, 80, 6), pos(154, 84, 154), ROOF)?;

    // Interior partitions on both floors.
    volume.fill_box(pos(78, 2, 10), pos(80, 40, 70), PARTITION)?;
    volume.fill_box(pos(78, 42, 10), pos(80, 80, 70), PARTITION)?;
    volume.fill_box(pos(78, 2, 30), pos(80, 26, 42), crate::AIR)?;
    volume.fill_box(pos(78, 42, 30), pos(80, 66, 42), crate::AIR)?;

    // Front door on the south wall (z = 8..10).
    volume.fill_box(pos(68, 2, 8), pos(80, 26, 10), crate::AIR)?;

    // Windows on both floors of all four walls.
    for y in [16, 52] {
        for (x0, x1) in [(32, 48), (104, 120)] {
            volume.fill_box(pos(x0, y, 8), pos(x1, y + 16, 10), crate::AIR)?;
            volume.fill_box(pos(x0, y, 150), pos(x1, y + 16, 152), crate::AIR)?;
        }
        for (z0, z1) in [(32, 48), (104, 120)] {
            volume.fill_box(pos(8, y, z0), pos(10, y + 16, z1), crate::AIR)?;
            volume.fill_box(pos(150, y, z0), pos(152, y + 16, z1), crate::AIR)?;
        }
    }

    // Straight stair inside the opening: 19 steps of 2 voxels rise and depth.
    for step in 0..19_u32 {
        let z = 20 + step * 2;
        volume.fill_box(pos(112, 2, z), pos(128, 4 + step * 2, z + 2), PARTITION)?;
    }

    Ok(())
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

    #[test]
    fn two_story_house_is_deterministic() {
        let first = two_story_house().unwrap();
        let second = two_story_house().unwrap();
        assert_eq!(first.config(), VolumeConfig::default());
        assert!(!first.is_empty());

        // Golden values shared with the volume and slice-house benchmarks.
        // Changing the fixture must update these and the limits in
        // `docs/39 - Slice House Benchmark.md`.
        assert_eq!(first.solid_count(), 266_432);
        assert_eq!(second.solid_count(), first.solid_count());

        let first_mesh = first.mesh();
        let second_mesh = second.mesh();
        assert_eq!(first_mesh.quad_count(), 293);
        assert_eq!(first_mesh.vertices.len(), 1_172);
        assert_eq!(first_mesh.indices.len(), 1_758);
        assert_eq!(first_mesh, second_mesh);
        assert_eq!(first_mesh.checksum(), 0x6fb7_acce_9894_19b7);
    }
}
