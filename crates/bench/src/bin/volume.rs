//! Deterministic 2-story voxel volume mesh benchmark.
//!
//! Builds a shell with two floors, rooms, a door, windows, and a staircase in
//! the default 5 × 3 × 5 chunk volume (16 m × 9.6 m × 16 m), then meshes the
//! whole volume repeatedly. Run with `--release` for the reported numbers.

use std::hint::black_box;
use std::time::Instant;

use core_voxel::demo::{ROOF, STONE, WOOD};
use core_voxel::error::VoxelError;
use core_voxel::volume::{VolumeConfig, VolumePos, VoxelVolume};
use core_voxel::MaterialId;

const PARTITION: MaterialId = 4;
const WARMUP_ITERATIONS: usize = 3;
const MEASURED_ITERATIONS: usize = 20;

fn pos(x: u32, y: u32, z: u32) -> VolumePos {
    VolumePos::new(x, y, z)
}

fn build_house(volume: &mut VoxelVolume) -> Result<(), VoxelError> {
    // Ground floor slab.
    volume.fill_box(pos(8, 0, 8), pos(152, 2, 152), WOOD)?;

    // Perimeter shell, then hollow the interior.
    volume.fill_box(pos(8, 2, 8), pos(152, 80, 152), STONE)?;
    volume.fill_box(pos(10, 2, 10), pos(150, 80, 150), core_voxel::AIR)?;

    // First-floor slab with a stairwell opening.
    volume.fill_box(pos(10, 40, 10), pos(150, 42, 150), WOOD)?;
    volume.fill_box(pos(112, 40, 20), pos(136, 42, 58), core_voxel::AIR)?;

    // Roof with a 0.2 m overhang.
    volume.fill_box(pos(6, 80, 6), pos(154, 84, 154), ROOF)?;

    // Interior partitions on both floors.
    volume.fill_box(pos(78, 2, 10), pos(80, 40, 70), PARTITION)?;
    volume.fill_box(pos(78, 42, 10), pos(80, 80, 70), PARTITION)?;
    volume.fill_box(pos(78, 2, 30), pos(80, 26, 42), core_voxel::AIR)?;
    volume.fill_box(pos(78, 42, 30), pos(80, 66, 42), core_voxel::AIR)?;

    // Front door on the south wall (z = 8..10).
    volume.fill_box(pos(68, 2, 8), pos(80, 26, 10), core_voxel::AIR)?;

    // Windows on both floors of all four walls.
    for y in [16, 52] {
        for (x0, x1) in [(32, 48), (104, 120)] {
            volume.fill_box(pos(x0, y, 8), pos(x1, y + 16, 10), core_voxel::AIR)?;
            volume.fill_box(pos(x0, y, 150), pos(x1, y + 16, 152), core_voxel::AIR)?;
        }
        for (z0, z1) in [(32, 48), (104, 120)] {
            volume.fill_box(pos(8, y, z0), pos(10, y + 16, z1), core_voxel::AIR)?;
            volume.fill_box(pos(150, y, z0), pos(152, y + 16, z1), core_voxel::AIR)?;
        }
    }

    // Straight stair inside the opening: 19 steps of 2 voxels rise and depth.
    for step in 0..19_u32 {
        let z = 20 + step * 2;
        volume.fill_box(pos(112, 2, z), pos(128, 4 + step * 2, z + 2), PARTITION)?;
    }

    Ok(())
}

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = samples.len() / 2;
    if samples.len().is_multiple_of(2) {
        (samples[mid - 1] + samples[mid]) / 2.0
    } else {
        samples[mid]
    }
}

fn main() {
    let config = VolumeConfig::default();
    let mut volume = VoxelVolume::new(config).expect("default volume config is valid");
    build_house(&mut volume).expect("deterministic house fits the default volume");

    for _ in 0..WARMUP_ITERATIONS {
        black_box(volume.mesh());
    }

    let mut samples = Vec::with_capacity(MEASURED_ITERATIONS);
    let mut mesh = None;
    for _ in 0..MEASURED_ITERATIONS {
        let start = Instant::now();
        let measured = black_box(volume.mesh());
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        mesh = Some(measured);
    }

    let mesh = mesh.expect("at least one measured iteration");
    let median_ms = median(&mut samples);
    let min_ms = samples.first().copied().unwrap_or_default();
    let max_ms = samples.last().copied().unwrap_or_default();
    let chunks = config.chunks();
    let voxels = config.voxels();
    let bounds = config.bounds_m();

    println!(
        concat!(
            "{{\"scene\":\"volume_two_story\",\"chunks\":[{},{},{}],",
            "\"voxels\":[{},{},{}],\"bounds_m\":[{:.1},{:.1},{:.1}],",
            "\"solid_voxels\":{},\"chunks_allocated\":{},",
            "\"quads\":{},\"triangles\":{},\"vertices\":{},\"indices\":{},",
            "\"iterations\":{},\"warmup\":{},",
            "\"median_mesh_ms\":{:.4},\"min_mesh_ms\":{:.4},\"max_mesh_ms\":{:.4},",
            "\"checksum\":\"{:016x}\"}}"
        ),
        chunks[0],
        chunks[1],
        chunks[2],
        voxels[0],
        voxels[1],
        voxels[2],
        bounds[0],
        bounds[1],
        bounds[2],
        volume.solid_count(),
        volume.chunks_allocated(),
        mesh.quad_count(),
        mesh.indices.len() / 3,
        mesh.vertices.len(),
        mesh.indices.len(),
        MEASURED_ITERATIONS,
        WARMUP_ITERATIONS,
        median_ms,
        min_ms,
        max_ms,
        mesh.checksum()
    );
}
