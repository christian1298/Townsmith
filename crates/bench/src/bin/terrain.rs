//! Phase 0 terrain mesh microbenchmark.

use std::hint::black_box;
use std::time::Instant;

use core_world::{
    terrain_mesh_lod, WorldChunk, WorldChunkCoord, WorldGenerator, WORLD_CHUNK_CELLS,
};

struct Measurement {
    median_ms: f64,
    min_ms: f64,
    max_ms: f64,
    quads: usize,
    vertices: usize,
    indices: usize,
}

fn measure(chunk: &WorldChunk, step: usize, iterations: usize) -> Measurement {
    let mut samples = Vec::with_capacity(iterations);
    let mut quads = 0;
    let mut vertices = 0;
    let mut indices = 0;

    for _ in 0..iterations {
        let start = Instant::now();
        let mesh = black_box(terrain_mesh_lod(black_box(chunk), step));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        quads = mesh.quad_count();
        vertices = mesh.vertices.len();
        indices = mesh.indices.len();
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    Measurement {
        median_ms: samples[samples.len() / 2],
        min_ms: samples.first().copied().unwrap_or_default(),
        max_ms: samples.last().copied().unwrap_or_default(),
        quads,
        vertices,
        indices,
    }
}

fn main() {
    let seed = 2026;
    let generator = WorldGenerator::new(seed);
    let chunk = generator
        .generate(WorldChunkCoord::new(0, 0))
        .expect("generated chunk");

    let iterations = 20usize;
    let full = measure(&chunk, 1, iterations);
    let half = measure(&chunk, 2, iterations);
    let quarter = measure(&chunk, 4, iterations);

    println!(
        concat!(
            "{{\"scene\":\"terrain_mesh\",\"seed\":{},\"cells\":{},",
            "\"quads\":{},\"vertices\":{},\"indices\":{},",
            "\"iterations\":{},\"median_mesh_ms\":{:.3},\"min_mesh_ms\":{:.3},",
            "\"max_mesh_ms\":{:.3},",
            "\"lod_2_quads\":{},\"lod_2_median_ms\":{:.3},",
            "\"lod_4_quads\":{},\"lod_4_median_ms\":{:.3}}}"
        ),
        seed,
        WORLD_CHUNK_CELLS,
        full.quads,
        full.vertices,
        full.indices,
        iterations,
        full.median_ms,
        full.min_ms,
        full.max_ms,
        half.quads,
        half.median_ms,
        quarter.quads,
        quarter.median_ms
    );
}
