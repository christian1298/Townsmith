//! Chunk streaming around a player or camera.
//!
//! The streaming manager owns the loaded terrain chunks and the delta store.
//! It is deliberately headless: rendering and collision consume its output.

use std::collections::{BTreeMap, BTreeSet};

use crate::biomes::Biome;
use crate::chunk::WorldChunk;
use crate::coords::{TerrainPos, WorldChunkCoord};
use crate::delta::DeltaStore;
use crate::error::WorldError;
use crate::generator::WorldGenerator;

/// Streaming radius and unload hysteresis, measured in 256 m chunks.
///
/// Values are sanitized by [`Self::sanitized`] when a [`StreamingWorld`] is
/// created, so a negative radius cannot unload everything and a huge radius
/// cannot synchronously generate an unbounded set of 192 KiB chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamingConfig {
    /// Load chunks within this Chebyshev radius, `0..=MAX_VIEW_RADIUS`.
    pub view_radius: i64,
    /// Keep chunks until they are this many chunks beyond the view radius,
    /// `0..=MAX_UNLOAD_MARGIN`.
    pub unload_margin: i64,
}

impl StreamingConfig {
    /// Largest accepted view radius (17 x 17 chunks, ~4.4 km across).
    pub const MAX_VIEW_RADIUS: i64 = 8;
    /// Largest accepted unload hysteresis.
    pub const MAX_UNLOAD_MARGIN: i64 = 8;

    /// Clamp both fields to their supported ranges.
    ///
    /// The worst-case resident set is
    /// `(2 * MAX_VIEW_RADIUS + MAX_UNLOAD_MARGIN + 1)^2` chunks (625, ~117 MiB
    /// of dense height/biome cells), not an unbounded request.
    #[must_use]
    pub const fn sanitized(self) -> Self {
        Self {
            view_radius: clamp_i64(self.view_radius, 0, Self::MAX_VIEW_RADIUS),
            unload_margin: clamp_i64(self.unload_margin, 0, Self::MAX_UNLOAD_MARGIN),
        }
    }
}

const fn clamp_i64(value: i64, min: i64, max: i64) -> i64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            view_radius: 2,
            unload_margin: 1,
        }
    }
}

/// Chunks loaded and unloaded by one update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamUpdate {
    /// Newly loaded chunks.
    pub loaded: Vec<WorldChunkCoord>,
    /// Chunks removed from memory.
    pub unloaded: Vec<WorldChunkCoord>,
}

impl StreamUpdate {
    /// Whether this update changed the loaded set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty() && self.unloaded.is_empty()
    }
}

/// Loaded terrain chunks plus persistent edit deltas.
#[derive(Debug, Clone)]
pub struct StreamingWorld {
    generator: WorldGenerator,
    config: StreamingConfig,
    loaded: BTreeMap<WorldChunkCoord, WorldChunk>,
    deltas: DeltaStore,
}

impl StreamingWorld {
    /// Create an empty streaming world.
    #[must_use]
    pub fn new(seed: u64, config: StreamingConfig) -> Self {
        Self {
            generator: WorldGenerator::new(seed),
            config: config.sanitized(),
            loaded: BTreeMap::new(),
            deltas: DeltaStore::new(),
        }
    }

    /// The world generator.
    #[must_use]
    pub const fn generator(&self) -> &WorldGenerator {
        &self.generator
    }

    /// The streaming configuration.
    #[must_use]
    pub const fn config(&self) -> StreamingConfig {
        self.config
    }

    /// The persistent delta store.
    #[must_use]
    pub const fn deltas(&self) -> &DeltaStore {
        &self.deltas
    }

    /// Number of loaded chunks.
    #[must_use]
    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    /// Whether a chunk is loaded.
    #[must_use]
    pub fn is_loaded(&self, coord: WorldChunkCoord) -> bool {
        self.loaded.contains_key(&coord)
    }

    /// A loaded chunk.
    #[must_use]
    pub fn chunk(&self, coord: WorldChunkCoord) -> Option<&WorldChunk> {
        self.loaded.get(&coord)
    }

    /// Load missing chunks around a position and unload far chunks.
    pub fn update(&mut self, player: TerrainPos) -> Result<StreamUpdate, WorldError> {
        let config = self.config.sanitized();
        let center = WorldChunkCoord::from_terrain_pos(player);
        let radius = config.view_radius;
        let mut update = StreamUpdate::default();

        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let coord =
                    WorldChunkCoord::new(center.x.saturating_add(dx), center.z.saturating_add(dz));
                // Chunks whose world origin does not fit in `i64` cannot be
                // addressed by any `TerrainPos`; skip them near the coordinate
                // limits instead of generating an unrepresentable chunk.
                if coord.checked_origin_m().is_none() {
                    continue;
                }
                if self.loaded.contains_key(&coord) {
                    continue;
                }
                let mut chunk = self.generator.generate(coord)?;
                self.deltas.apply_to(&mut chunk);
                self.loaded.insert(coord, chunk);
                update.loaded.push(coord);
            }
        }

        let unload_radius = radius.saturating_add(config.unload_margin);
        let to_unload: Vec<_> = self
            .loaded
            .keys()
            .copied()
            .filter(|coord| coord.distance_chebyshev(center) > unload_radius)
            .collect();
        for coord in to_unload {
            self.loaded.remove(&coord);
            update.unloaded.push(coord);
        }

        update.loaded.sort();
        update.unloaded.sort();
        Ok(update)
    }

    /// Height at a world position, or `None` when its chunk is not loaded.
    #[must_use]
    pub fn height_at(&self, pos: TerrainPos) -> Option<u16> {
        let coord = WorldChunkCoord::from_terrain_pos(pos);
        let chunk = self.loaded.get(&coord)?;
        let (x, z) = coord.local_from_world(pos)?;
        chunk.height(x, z)
    }

    /// Biome at a world position, or `None` when its chunk is not loaded.
    #[must_use]
    pub fn biome_at(&self, pos: TerrainPos) -> Option<Biome> {
        let coord = WorldChunkCoord::from_terrain_pos(pos);
        let chunk = self.loaded.get(&coord)?;
        let (x, z) = coord.local_from_world(pos)?;
        chunk.biome(x, z)
    }

    /// Edit terrain height. The delta is recorded even if the chunk is unloaded.
    pub fn apply_edit(&mut self, pos: TerrainPos, height: u16) -> Result<(), WorldError> {
        let coord = WorldChunkCoord::from_terrain_pos(pos);
        let (x, z) = coord.local_from_world(pos).ok_or(WorldError::OutOfBounds {
            x: pos.x.unsigned_abs() as usize,
            z: pos.z.unsigned_abs() as usize,
        })?;
        self.deltas.set_height(coord, x as u8, z as u8, height)?;
        if let Some(chunk) = self.loaded.get_mut(&coord) {
            chunk.set_height(x, z, height)?;
        }
        Ok(())
    }

    /// Encode all terrain edits for persistence or network transfer.
    #[must_use]
    pub fn encode_deltas(&self) -> Vec<u8> {
        self.deltas.encode()
    }

    /// Replace the current deltas from an encoded blob.
    ///
    /// The blob is decoded before anything changes. Then every loaded chunk is
    /// regenerated from the world generator and the new deltas are applied, so
    /// edits that existed only in the previous store disappear instead of
    /// leaving loaded terrain a mix of old and new. On a decode error the
    /// world (loaded chunks and deltas) is untouched.
    pub fn load_deltas(&mut self, bytes: &[u8]) -> Result<(), WorldError> {
        let deltas = DeltaStore::decode(bytes)?;

        let mut regenerated = BTreeMap::new();
        for &coord in self.loaded.keys() {
            let mut chunk = self.generator.generate(coord)?;
            deltas.apply_to(&mut chunk);
            regenerated.insert(coord, chunk);
        }

        self.deltas = deltas;
        self.loaded = regenerated;
        Ok(())
    }

    /// Loaded chunk coordinates, useful for tests and debug overlays.
    #[must_use]
    pub fn loaded_coords(&self) -> BTreeSet<WorldChunkCoord> {
        self.loaded.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(radius: i64, margin: i64) -> StreamingConfig {
        StreamingConfig {
            view_radius: radius,
            unload_margin: margin,
        }
    }

    #[test]
    fn loads_the_initial_view() {
        let mut world = StreamingWorld::new(42, config(1, 0));
        let update = world.update(TerrainPos::new(0, 0)).expect("update");
        assert_eq!(update.loaded.len(), 9);
        assert!(update.unloaded.is_empty());
        assert_eq!(world.loaded_count(), 9);
        assert!(world.is_loaded(WorldChunkCoord::new(-1, -1)));
        assert!(world.is_loaded(WorldChunkCoord::new(1, 1)));
    }

    #[test]
    fn loads_and_unloads_when_moving() {
        let mut world = StreamingWorld::new(42, config(1, 0));
        world.update(TerrainPos::new(0, 0)).expect("update");
        let update = world.update(TerrainPos::new(256, 0)).expect("update");
        assert_eq!(update.loaded.len(), 3);
        assert_eq!(update.unloaded.len(), 3);
        assert_eq!(world.loaded_count(), 9);
        assert!(world.is_loaded(WorldChunkCoord::new(2, 0)));
        assert!(!world.is_loaded(WorldChunkCoord::new(-1, 0)));
    }

    #[test]
    fn hysteresis_keeps_chunks_longer() {
        let mut world = StreamingWorld::new(42, config(1, 1));
        world.update(TerrainPos::new(0, 0)).expect("update");
        let update = world.update(TerrainPos::new(256, 0)).expect("update");
        assert_eq!(update.loaded.len(), 3);
        assert!(update.unloaded.is_empty());
        assert!(world.is_loaded(WorldChunkCoord::new(-1, 0)));
    }

    #[test]
    fn edits_survive_unload_and_reload() {
        let mut world = StreamingWorld::new(7, config(0, 0));
        let pos = TerrainPos::new(10, 10);
        world.update(pos).expect("update");
        world.apply_edit(pos, 77).expect("edit");
        assert_eq!(world.height_at(pos), Some(77));

        world.update(TerrainPos::new(256, 0)).expect("move away");
        assert_eq!(world.height_at(pos), None);

        world.update(pos).expect("move back");
        assert_eq!(world.height_at(pos), Some(77));
        assert_eq!(world.deltas().edit_count(), 1);
    }

    #[test]
    fn edits_to_unloaded_chunks_are_recorded() {
        let mut world = StreamingWorld::new(7, config(0, 0));
        let pos = TerrainPos::new(300, -20);
        world.apply_edit(pos, 60).expect("edit");
        assert_eq!(world.deltas().edit_count(), 1);
        world.update(pos).expect("load");
        assert_eq!(world.height_at(pos), Some(60));
    }

    #[test]
    fn delta_blob_round_trips_through_streaming() {
        let mut world = StreamingWorld::new(11, config(0, 0));
        let pos = TerrainPos::new(-5, 9);
        world.update(pos).expect("load");
        world.apply_edit(pos, 55).expect("edit");
        let encoded = world.encode_deltas();

        let mut restored = StreamingWorld::new(11, config(0, 0));
        restored.load_deltas(&encoded).expect("load deltas");
        restored.update(pos).expect("load");
        assert_eq!(restored.height_at(pos), Some(55));
    }

    #[test]
    fn height_and_biome_are_none_outside_the_loaded_view() {
        let mut world = StreamingWorld::new(5, config(0, 0));
        world.update(TerrainPos::new(0, 0)).expect("update");
        assert!(world.height_at(TerrainPos::new(0, 0)).is_some());
        assert!(world.height_at(TerrainPos::new(256, 0)).is_none());
        assert!(world.biome_at(TerrainPos::new(0, 0)).is_some());
    }

    #[test]
    fn a_second_update_without_movement_is_empty() {
        let mut world = StreamingWorld::new(5, config(1, 0));
        world.update(TerrainPos::new(0, 0)).expect("update");
        let update = world.update(TerrainPos::new(10, 10)).expect("update");
        assert!(update.is_empty());
    }

    #[test]
    fn config_is_sanitized_at_construction() {
        let negative = StreamingWorld::new(1, config(-5, -3));
        assert_eq!(negative.config(), config(0, 0));

        let huge = StreamingWorld::new(1, config(i64::MAX, i64::MAX));
        assert_eq!(
            huge.config(),
            config(
                StreamingConfig::MAX_VIEW_RADIUS,
                StreamingConfig::MAX_UNLOAD_MARGIN
            )
        );
        assert_eq!(
            StreamingConfig::default().sanitized(),
            StreamingConfig::default()
        );
    }

    #[test]
    fn negative_radius_still_loads_the_center_chunk() {
        let mut world = StreamingWorld::new(42, config(-4, -4));
        let update = world.update(TerrainPos::new(0, 0)).expect("update");
        assert_eq!(update.loaded, vec![WorldChunkCoord::new(0, 0)]);
        assert!(update.unloaded.is_empty());
        assert_eq!(world.loaded_count(), 1);
    }

    #[test]
    fn huge_unload_margin_is_bounded() {
        let mut world = StreamingWorld::new(42, config(0, i64::MAX));
        assert_eq!(
            world.config().unload_margin,
            StreamingConfig::MAX_UNLOAD_MARGIN
        );
        world.update(TerrainPos::new(0, 0)).expect("update");
        let update = world.update(TerrainPos::new(20 * 256, 0)).expect("update");
        assert_eq!(update.loaded, vec![WorldChunkCoord::new(20, 0)]);
        assert_eq!(update.unloaded, vec![WorldChunkCoord::new(0, 0)]);
    }

    #[test]
    fn huge_view_radius_is_capped_before_loading() {
        let huge = config(i64::MAX, i64::MAX);
        let sanitized = huge.sanitized();
        assert_eq!(sanitized.view_radius, StreamingConfig::MAX_VIEW_RADIUS);
        assert_eq!(sanitized.unload_margin, StreamingConfig::MAX_UNLOAD_MARGIN);
        assert_eq!(sanitized.sanitized(), sanitized);

        // Worst-case resident set: (2r + margin + 1)^2 = 625 chunks, ~117 MiB.
        let side = 2 * sanitized.view_radius + sanitized.unload_margin + 1;
        assert_eq!(side * side, 625);
        let world = StreamingWorld::new(42, huge);
        assert_eq!(world.config(), sanitized);
    }

    #[test]
    fn loading_new_deltas_removes_old_only_edits() {
        let mut world = StreamingWorld::new(7, config(0, 0));
        let origin = WorldChunkCoord::new(0, 0).origin_m();
        let kept = TerrainPos::new(origin.x + 10, origin.z + 10);
        let old_only = TerrainPos::new(origin.x + 20, origin.z + 20);
        world.update(kept).expect("load");
        world.apply_edit(kept, 77).expect("edit");
        world.apply_edit(old_only, 66).expect("edit");
        assert_eq!(world.height_at(old_only), Some(66));

        let mut next = DeltaStore::new();
        next.set_height(WorldChunkCoord::new(0, 0), 10, 10, 55)
            .expect("edit");
        world.load_deltas(&next.encode()).expect("replace deltas");

        assert_eq!(world.deltas().edit_count(), 1);
        assert_eq!(world.loaded_count(), 1);
        assert_eq!(world.height_at(kept), Some(55));
        assert_eq!(
            world.height_at(old_only),
            Some(world.generator().height_at(old_only))
        );
    }

    #[test]
    fn failed_delta_load_leaves_the_world_untouched() {
        let mut world = StreamingWorld::new(7, config(0, 0));
        let pos = TerrainPos::new(10, 10);
        world.update(pos).expect("load");
        world.apply_edit(pos, 77).expect("edit");
        let checksum = world.deltas().checksum();

        assert_eq!(
            world.load_deltas(b"nope"),
            Err(WorldError::InvalidDeltaMagic)
        );
        assert_eq!(world.height_at(pos), Some(77));
        assert_eq!(world.deltas().edit_count(), 1);
        assert_eq!(world.deltas().checksum(), checksum);
    }
}
