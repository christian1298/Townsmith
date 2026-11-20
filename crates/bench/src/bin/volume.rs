//! Deterministic 2-story voxel volume mesh benchmark.
//!
//! Builds a shell with two floors, rooms, a door, windows, and a staircase in
//! the default 5 × 3 × 5 chunk volume (16 m × 9.6 m × 16 m), then meshes the
//! whole volume repeatedly. Run with `--release` for the reported numbers.

use std::hint::black_box;
use std::time::Instant;

use core_voxel::demo;

const WARMUP_ITERATIONS: usize = 3;
const MEASURED_ITERATIONS: usize = 20;

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
    let volume = demo::two_story_house().expect("deterministic house fits the default volume");
    let config = volume.config();

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
