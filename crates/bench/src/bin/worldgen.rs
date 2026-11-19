//! Phase 0 worldgen and streaming microbenchmark.
//!
//! This is a headless benchmark. It measures deterministic chunk generation,
//! streaming around the origin, terrain edits, delta encoding, and reload.

use std::time::Instant;

use core_world::{
    Biome, StreamingConfig, StreamingWorld, TerrainPos, WorldChunkCoord, WORLD_CHUNK_CELLS,
};

fn main() {
    let seed = 0x7057_6E73_6D69_7468;
    let config = StreamingConfig {
        view_radius: 1,
        unload_margin: 1,
    };

    let mut world = StreamingWorld::new(seed, config);
    let start = Instant::now();
    let update = world.update(TerrainPos::new(0, 0)).expect("initial stream");
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;

    let loaded_chunks = world.loaded_count();
    let mut min_height = u16::MAX;
    let mut max_height = 0u16;
    let mut biome_counts = [0usize; 5];
    let mut memory_bytes = 0usize;

    for coord in world.loaded_coords() {
        let chunk = world.chunk(coord).expect("loaded chunk");
        min_height = min_height.min(chunk.min_height());
        max_height = max_height.max(chunk.max_height());
        memory_bytes += chunk.memory_bytes();
        for &biome in chunk.biomes() {
            biome_counts[biome as usize] += 1;
        }
    }

    let origin = WorldChunkCoord::new(0, 0).origin_m();
    let edit_count = 2_000u32;
    let start = Instant::now();
    for i in 0..edit_count {
        let x = (i % 256) as i64;
        let z = ((i / 256) % 256) as i64;
        let height = 30 + (i % 40) as u16;
        world
            .apply_edit(TerrainPos::new(origin.x + x, origin.z + z), height)
            .expect("edit");
    }
    let edit_ms = start.elapsed().as_secs_f64() * 1000.0;
    let delta_bytes = world.encode_deltas();

    let start = Instant::now();
    let mut restored = StreamingWorld::new(seed, config);
    restored.load_deltas(&delta_bytes).expect("load deltas");
    restored
        .update(TerrainPos::new(0, 0))
        .expect("reload stream");
    let reload_ms = start.elapsed().as_secs_f64() * 1000.0;

    println!(
        concat!(
            "{{\"scene\":\"worldgen_streaming\",\"seed\":{},\"view_radius\":{},",
            "\"loaded_chunks\":{},\"loaded_cells\":{},\"loaded_updates\":{},",
            "\"unloaded_updates\":{},\"load_ms\":{:.3},",
            "\"height_min\":{},\"height_max\":{},\"biome_plains\":{},",
            "\"biome_forest\":{},\"biome_hills\":{},\"biome_coast\":{},",
            "\"biome_river_valley\":{},\"memory_bytes\":{},",
            "\"edits\":{},\"edit_ms\":{:.3},\"delta_bytes\":{},",
            "\"reload_ms\":{:.3},\"reloaded_deltas\":{}}}"
        ),
        seed,
        config.view_radius,
        loaded_chunks,
        loaded_chunks * WORLD_CHUNK_CELLS,
        update.loaded.len(),
        update.unloaded.len(),
        load_ms,
        min_height,
        max_height,
        biome_counts[Biome::Plains as usize],
        biome_counts[Biome::Forest as usize],
        biome_counts[Biome::Hills as usize],
        biome_counts[Biome::Coast as usize],
        biome_counts[Biome::RiverValley as usize],
        memory_bytes,
        edit_count,
        edit_ms,
        delta_bytes.len(),
        reload_ms,
        restored.deltas().edit_count()
    );
}
