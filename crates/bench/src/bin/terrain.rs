//! Phase 0 terrain mesh microbenchmark.

use std::hint::black_box;
use std::time::Instant;

use core_world::{terrain_mesh, WorldChunkCoord, WorldGenerator, WORLD_CHUNK_CELLS};

fn main() {
    let seed = 2026;
    let generator = WorldGenerator::new(seed);
    let chunk = generator
        .generate(WorldChunkCoord::new(0, 0))
        .expect("generated chunk");

    let iterations = 20usize;
    let mut samples = Vec::with_capacity(iterations);
    let mut quads = 0;
    let mut vertices = 0;
    let mut indices = 0;

    for _ in 0..iterations {
        let start = Instant::now();
        let mesh = black_box(terrain_mesh(black_box(&chunk)));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        quads = mesh.quad_count();
        vertices = mesh.vertices.len();
        indices = mesh.indices.len();
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = samples[samples.len() / 2];

    println!(
        concat!(
            "{{\"scene\":\"terrain_mesh\",\"seed\":{},\"cells\":{},",
            "\"quads\":{},\"vertices\":{},\"indices\":{},",
            "\"iterations\":{},\"median_mesh_ms\":{:.3},\"min_mesh_ms\":{:.3},",
            "\"max_mesh_ms\":{:.3}}}"
        ),
        seed,
        WORLD_CHUNK_CELLS,
        quads,
        vertices,
        indices,
        iterations,
        median,
        samples.first().copied().unwrap_or_default(),
        samples.last().copied().unwrap_or_default()
    );
}
