//! Phase 0 worldgen and streaming microbenchmark.
//!
//! This is a headless benchmark. It measures deterministic chunk generation,
//! streaming around the origin, terrain edits, delta encoding, and reload.
//!
//! The reload is gated: after `load_deltas` the harness checks the restored
//! edit count, spot-checks edited heights, and compares an FNV-1a checksum over
//! every loaded height and biome. A `load_deltas` or apply-to-chunk regression
//! fails the run instead of only printing a number.

use std::time::Instant;

use core_world::{
    Biome, StreamingConfig, StreamingWorld, TerrainPos, WorldChunkCoord, WORLD_CHUNK_CELLS,
};

const SEED: u64 = 0x7057_6E73_6D69_7468;
const VIEW_RADIUS: i64 = 1;
const UNLOAD_MARGIN: i64 = 1;
const EDIT_COUNT: u32 = 2_000;
const EDIT_BASE_HEIGHT: u16 = 30;
const EDIT_HEIGHT_SPAN: u16 = 40;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn main() {
    let config = config();

    let mut world = StreamingWorld::new(SEED, config);
    let start = Instant::now();
    let update = world.update(TerrainPos::new(0, 0)).expect("initial stream");
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;

    let loaded_chunks = world.loaded_count();
    let mut min_height = u16::MAX;
    let mut max_height = 0u16;
    let mut biome_counts = [0usize; Biome::ALL.len()];
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

    let start = Instant::now();
    for index in 0..EDIT_COUNT {
        let (pos, height) = edit_at(index);
        world.apply_edit(pos, height).expect("edit");
    }
    let edit_ms = start.elapsed().as_secs_f64() * 1000.0;
    let delta_bytes = world.encode_deltas();

    let start = Instant::now();
    let mut restored = StreamingWorld::new(SEED, config);
    restored.load_deltas(&delta_bytes).expect("load deltas");
    restored
        .update(TerrainPos::new(0, 0))
        .expect("reload stream");
    let reload_ms = start.elapsed().as_secs_f64() * 1000.0;

    if let Err(error) = verify_reload(&world, &restored, EDIT_COUNT) {
        panic!("worldgen reload gate failed: {error}");
    }
    let state_checksum = state_checksum(&world);

    println!(
        concat!(
            "{{\"scene\":\"worldgen_streaming\",\"seed\":{},\"view_radius\":{},",
            "\"loaded_chunks\":{},\"loaded_cells\":{},\"loaded_updates\":{},",
            "\"unloaded_updates\":{},\"load_ms\":{:.3},",
            "\"height_min\":{},\"height_max\":{},\"biome_plains\":{},",
            "\"biome_forest\":{},\"biome_hills\":{},\"biome_coast\":{},",
            "\"biome_river_valley\":{},\"memory_bytes\":{},",
            "\"edits\":{},\"edit_ms\":{:.3},\"delta_bytes\":{},",
            "\"reload_ms\":{:.3},\"reloaded_deltas\":{},",
            "\"state_checksum\":\"{:016x}\",\"reload_verified\":true}}"
        ),
        SEED,
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
        EDIT_COUNT,
        edit_ms,
        delta_bytes.len(),
        reload_ms,
        restored.deltas().edit_count(),
        state_checksum
    );
}

const fn config() -> StreamingConfig {
    StreamingConfig {
        view_radius: VIEW_RADIUS,
        unload_margin: UNLOAD_MARGIN,
    }
}

/// The deterministic edit at `index`: positions stay in the origin chunk while
/// the count is below 65,536, and heights cycle through 30..=69.
fn edit_at(index: u32) -> (TerrainPos, u16) {
    let origin = WorldChunkCoord::new(0, 0).origin_m();
    let x = i64::from(index % 256);
    let z = i64::from((index / 256) % 256);
    let height = EDIT_BASE_HEIGHT + (index % u32::from(EDIT_HEIGHT_SPAN)) as u16;
    (TerrainPos::new(origin.x + x, origin.z + z), height)
}

/// Assert that a reloaded world reproduces the edited source exactly: delta
/// count, loaded set, spot heights, and the full height/biome checksum.
fn verify_reload(
    source: &StreamingWorld,
    restored: &StreamingWorld,
    edit_count: u32,
) -> Result<(), String> {
    let expected_edits = edit_count as usize;
    let actual_edits = restored.deltas().edit_count();
    if actual_edits != expected_edits {
        return Err(format!(
            "reloaded delta store has {actual_edits} edits, expected {expected_edits}"
        ));
    }

    let source_coords = source.loaded_coords();
    let restored_coords = restored.loaded_coords();
    if source_coords != restored_coords {
        return Err(format!(
            "reloaded chunk set has {} chunks, original has {}",
            restored_coords.len(),
            source_coords.len()
        ));
    }

    let mut spots = vec![0_u32, 1, 254, 255, 256, 257, edit_count.saturating_sub(1)];
    spots.retain(|&index| index < edit_count);
    spots.sort_unstable();
    spots.dedup();
    for index in spots {
        let (pos, expected) = edit_at(index);
        let source_height = source.height_at(pos);
        if source_height != Some(expected) {
            return Err(format!(
                "source height at edit {index} is {source_height:?}, expected {expected}"
            ));
        }
        let restored_height = restored.height_at(pos);
        if restored_height != Some(expected) {
            return Err(format!(
                "reloaded height at edit {index} is {restored_height:?}, expected {expected}"
            ));
        }
    }

    let source_checksum = state_checksum(source);
    let restored_checksum = state_checksum(restored);
    if source_checksum != restored_checksum {
        return Err(format!(
            "reloaded state checksum {restored_checksum:016x} != original {source_checksum:016x}"
        ));
    }
    Ok(())
}

/// FNV-1a over every loaded chunk's heights and biomes, in coordinate order.
fn state_checksum(world: &StreamingWorld) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for coord in world.loaded_coords() {
        let chunk = world.chunk(coord).expect("loaded chunk");
        for (height, biome) in chunk.heights().iter().zip(chunk.biomes()) {
            fnv1a_feed(&mut hash, u64::from(*height));
            fnv1a_feed(&mut hash, u64::from(*biome as u8));
        }
    }
    hash
}

fn fnv1a_feed(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restored_pair(edits: u32) -> (StreamingWorld, StreamingWorld) {
        let config = config();
        let mut world = StreamingWorld::new(SEED, config);
        world.update(TerrainPos::new(0, 0)).expect("stream");
        for index in 0..edits {
            let (pos, height) = edit_at(index);
            world.apply_edit(pos, height).expect("edit");
        }
        let bytes = world.encode_deltas();

        let mut restored = StreamingWorld::new(SEED, config);
        restored.load_deltas(&bytes).expect("load deltas");
        restored.update(TerrainPos::new(0, 0)).expect("reload");
        (world, restored)
    }

    #[test]
    fn reload_restores_edits_and_chunk_state() {
        let (world, restored) = restored_pair(300);
        assert_eq!(restored.deltas().edit_count(), 300);
        verify_reload(&world, &restored, 300).expect("reload gate");
    }

    #[test]
    fn reload_gate_rejects_a_world_without_deltas() {
        let (world, _) = restored_pair(64);
        let mut fresh = StreamingWorld::new(SEED, config());
        fresh.update(TerrainPos::new(0, 0)).expect("stream");
        let error = verify_reload(&world, &fresh, 64).expect_err("missing deltas must fail");
        assert!(error.contains("edits"), "unexpected error: {error}");
    }

    #[test]
    fn biome_counts_cover_every_variant() {
        for biome in Biome::ALL {
            assert!((biome as usize) < Biome::ALL.len());
        }
    }
}
