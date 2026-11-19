//! Phase 0 voxel microbenchmark.
//!
//! This is deliberately a microbenchmark, not the full in-game benchmark
//! harness. It proves the meshing path and gives a repeatable number to compare
//! against while the rest of the engine is built.

use std::hint::black_box;
use std::time::Instant;

use core_voxel::chunk::VoxelChunk;
use core_voxel::demo;
use core_voxel::mesh::greedy_mesh;
use core_voxel::CHUNK_SIZE;

fn checkerboard() -> VoxelChunk {
    let mut chunk = VoxelChunk::new();
    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                if (x + y + z) % 2 == 0 {
                    chunk.set(x, y, z, ((x + y + z) % 8 + 1) as u16).unwrap();
                }
            }
        }
    }
    chunk
}

fn timed_mesh(chunk: &VoxelChunk, iterations: usize) -> (Vec<f64>, usize, usize, usize) {
    let mut samples = Vec::with_capacity(iterations);
    let mut vertices = 0;
    let mut indices = 0;
    let mut quads = 0;

    for _ in 0..iterations {
        let start = Instant::now();
        let mesh = black_box(greedy_mesh(black_box(chunk)));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        vertices = mesh.vertices.len();
        indices = mesh.indices.len();
        quads = mesh.quad_count();
    }

    (samples, vertices, indices, quads)
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(50);

    let scenes = [
        ("checkerboard", checkerboard()),
        ("hut", demo::hut().expect("demo hut")),
    ];
    let mut rows = Vec::new();

    for (name, chunk) in scenes {
        let encoded = chunk.encode();
        let (mut samples, vertices, indices, quads) = timed_mesh(&chunk, iterations);
        let median_ms = median(&mut samples);
        let min_ms = samples.first().copied().unwrap_or_default();
        let max_ms = samples.last().copied().unwrap_or_default();

        rows.push(format!(
            concat!(
                "{{\"scene\":\"{}\",\"chunk_size\":{},\"solid_voxels\":{},",
                "\"palette_entries\":{},\"rle_runs\":{},\"vertices\":{},",
                "\"indices\":{},\"quads\":{},\"iterations\":{},",
                "\"median_mesh_ms\":{:.3},\"min_mesh_ms\":{:.3},\"max_mesh_ms\":{:.3}}}"
            ),
            name,
            CHUNK_SIZE,
            chunk.solid_count(),
            encoded.palette.len(),
            encoded.runs.len(),
            vertices,
            indices,
            quads,
            iterations,
            median_ms,
            min_ms,
            max_ms
        ));
    }

    println!("[{}]", rows.join(","));
}
