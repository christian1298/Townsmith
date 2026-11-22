//! Sparse player edits applied over the procedural terrain.
//!
//! The save format stores only differences from the generated world. This
//! module is deliberately independent of the save database: it is a compact,
//! deterministic binary blob that `core_save` can embed later.

use std::collections::BTreeMap;

use crate::chunk::WorldChunk;
use crate::coords::{TerrainPos, WorldChunkCoord, MAX_HEIGHT_M};
use crate::error::WorldError;
use crate::generator::WorldGenerator;

const DELTA_MAGIC: [u8; 4] = *b"TSDL";
const DELTA_VERSION: u16 = 1;

/// One terrain height edit within a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainEdit {
    /// Local X cell, 0..=255.
    pub x: u8,
    /// Local Z cell, 0..=255.
    pub z: u8,
    /// New height in meters.
    pub height: u16,
}

impl TerrainEdit {
    /// Create a terrain edit.
    #[must_use]
    pub const fn new(x: u8, z: u8, height: u16) -> Self {
        Self { x, z, height }
    }
}

/// Sparse edit deltas, grouped by chunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeltaStore {
    chunks: BTreeMap<WorldChunkCoord, ChunkEdits>,
}

/// Edits for one chunk: insertion order plus a cell index.
///
/// Encoding keeps the order edits were first recorded in, so valid blobs stay
/// byte-identical across the index change. Lookups and replacements go through
/// the `(x, z)` index instead of scanning the vector, which keeps full-chunk
/// terraform O(n log n) rather than O(n²).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ChunkEdits {
    order: Vec<TerrainEdit>,
    index: BTreeMap<(u8, u8), usize>,
}

impl ChunkEdits {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            order: Vec::with_capacity(capacity),
            index: BTreeMap::new(),
        }
    }

    fn len(&self) -> usize {
        self.order.len()
    }

    /// Insert or replace one cell. Returns `false` when the cell already existed.
    fn set(&mut self, x: u8, z: u8, height: u16) -> bool {
        if let Some(&position) = self.index.get(&(x, z)) {
            self.order[position].height = height;
            false
        } else {
            self.index.insert((x, z), self.order.len());
            self.order.push(TerrainEdit::new(x, z, height));
            true
        }
    }

    fn height(&self, x: u8, z: u8) -> Option<u16> {
        self.index
            .get(&(x, z))
            .map(|&position| self.order[position].height)
    }

    fn contains(&self, x: u8, z: u8) -> bool {
        self.index.contains_key(&(x, z))
    }

    fn iter(&self) -> std::slice::Iter<'_, TerrainEdit> {
        self.order.iter()
    }
}

impl DeltaStore {
    /// Create an empty delta store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a height edit. Repeating the same cell replaces the previous value.
    pub fn set_height(
        &mut self,
        coord: WorldChunkCoord,
        x: u8,
        z: u8,
        height: u16,
    ) -> Result<(), WorldError> {
        if height > MAX_HEIGHT_M {
            return Err(WorldError::InvalidHeight { height });
        }
        self.chunks.entry(coord).or_default().set(x, z, height);
        Ok(())
    }

    /// Height at a world position, using an edit when one exists.
    ///
    /// This is the delta-aware sampler used by terrain meshing and placement
    /// validation. It falls back to `generator.height_at` outside the edits.
    #[must_use]
    pub fn height_at(&self, generator: &WorldGenerator, pos: TerrainPos) -> u16 {
        let coord = WorldChunkCoord::from_terrain_pos(pos);
        if let Some((x, z)) = coord.local_from_world(pos) {
            if let Some(height) = self
                .chunks
                .get(&coord)
                .and_then(|edits| edits.height(x as u8, z as u8))
            {
                return height;
            }
        }
        generator.height_at(pos)
    }

    /// Apply the deltas for a chunk, if any.
    pub fn apply_to(&self, chunk: &mut WorldChunk) {
        if let Some(edits) = self.chunks.get(&chunk.coord()) {
            for edit in edits.iter() {
                // Local coordinates are u8, so they are always in range.
                let _ = chunk.set_height(edit.x as usize, edit.z as usize, edit.height);
            }
        }
    }

    /// Edits for one chunk, in insertion order.
    #[must_use]
    pub fn edits_for(&self, coord: WorldChunkCoord) -> &[TerrainEdit] {
        self.chunks
            .get(&coord)
            .map_or(&[], |edits| edits.order.as_slice())
    }

    /// Number of chunks with edits.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Total number of edits.
    #[must_use]
    pub fn edit_count(&self) -> usize {
        self.chunks.values().map(ChunkEdits::len).sum()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Stable checksum of an encoded delta blob.
    #[must_use]
    pub fn checksum_bytes(bytes: &[u8]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// Stable checksum of this store's encoded delta blob.
    #[must_use]
    pub fn checksum(&self) -> u64 {
        Self::checksum_bytes(&self.encode())
    }

    /// Size in bytes of this store's encoded blob.
    ///
    /// Layout: a 10-byte header, 20 bytes per edited chunk, 4 bytes per edit.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        10 + self.chunks.len() * 20 + self.edit_count() * 4
    }

    /// Encode to the phase 0 binary delta format.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&DELTA_MAGIC);
        out.extend_from_slice(&DELTA_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());

        for (coord, edits) in &self.chunks {
            out.extend_from_slice(&coord.x.to_le_bytes());
            out.extend_from_slice(&coord.z.to_le_bytes());
            out.extend_from_slice(&(edits.len() as u32).to_le_bytes());
            for edit in edits.iter() {
                out.push(edit.x);
                out.push(edit.z);
                out.extend_from_slice(&edit.height.to_le_bytes());
            }
        }
        out
    }

    /// Decode the phase 0 binary delta format.
    ///
    /// Duplicate chunk coordinates and duplicate `(x, z)` cells within a chunk
    /// are rejected. The encoder never produces them, so a valid blob
    /// re-encodes byte-identically.
    pub fn decode(bytes: &[u8]) -> Result<Self, WorldError> {
        let mut cursor = Cursor::new(bytes);
        if cursor.take::<4>()? != DELTA_MAGIC {
            return Err(WorldError::InvalidDeltaMagic);
        }
        let version = u16::from_le_bytes(cursor.take()?);
        if version != DELTA_VERSION {
            return Err(WorldError::UnsupportedDeltaVersion { version });
        }
        let chunk_count = u32::from_le_bytes(cursor.take()?) as usize;
        // Each chunk needs at least its 20-byte header. Bound the count before
        // allocating or iterating so a tiny malformed file cannot request a
        // huge allocation or a multi-billion-iteration loop.
        if chunk_count > cursor.remaining() / 20 {
            return Err(WorldError::TruncatedDelta);
        }
        let mut chunks = BTreeMap::new();

        for _ in 0..chunk_count {
            let x = i64::from_le_bytes(cursor.take()?);
            let z = i64::from_le_bytes(cursor.take()?);
            let coord = WorldChunkCoord::new(x, z);
            if chunks.contains_key(&coord) {
                return Err(WorldError::DuplicateDeltaChunk { x, z });
            }
            let edit_count = u32::from_le_bytes(cursor.take()?) as usize;
            if edit_count > cursor.remaining() / 4 {
                return Err(WorldError::TruncatedDelta);
            }
            let mut edits = ChunkEdits::with_capacity(edit_count);
            for _ in 0..edit_count {
                let edit_x = cursor.take::<1>()?[0];
                let edit_z = cursor.take::<1>()?[0];
                let height = u16::from_le_bytes(cursor.take()?);
                if height > MAX_HEIGHT_M {
                    return Err(WorldError::InvalidHeight { height });
                }
                if edits.contains(edit_x, edit_z) {
                    return Err(WorldError::DuplicateDeltaEdit {
                        chunk_x: x,
                        chunk_z: z,
                        x: edit_x,
                        z: edit_z,
                    });
                }
                edits.set(edit_x, edit_z, height);
            }
            chunks.insert(coord, edits);
        }

        if cursor.remaining() != 0 {
            return Err(WorldError::TrailingDeltaBytes {
                remaining: cursor.remaining(),
            });
        }
        Ok(Self { chunks })
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], WorldError> {
        let end = self
            .position
            .checked_add(N)
            .ok_or(WorldError::TruncatedDelta)?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(WorldError::TruncatedDelta)?;
        let mut array = [0u8; N];
        array.copy_from_slice(slice);
        self.position = end;
        Ok(array)
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biomes::Biome;
    use crate::coords::WORLD_CHUNK_CELLS;

    fn flat_chunk(coord: WorldChunkCoord) -> WorldChunk {
        WorldChunk::from_parts(
            coord,
            vec![24; WORLD_CHUNK_CELLS],
            vec![Biome::Plains; WORLD_CHUNK_CELLS],
        )
        .expect("valid chunk")
    }

    #[test]
    fn applies_edits_to_the_matching_chunk() {
        let coord = WorldChunkCoord::new(2, -1);
        let mut store = DeltaStore::new();
        store.set_height(coord, 1, 2, 80).expect("edit");
        store.set_height(coord, 255, 255, 40).expect("edit");
        let mut chunk = flat_chunk(coord);
        store.apply_to(&mut chunk);
        assert_eq!(chunk.height(1, 2), Some(80));
        assert_eq!(chunk.height(255, 255), Some(40));
        assert_eq!(chunk.height(0, 0), Some(24));
        assert_eq!(store.chunk_count(), 1);
        assert_eq!(store.edit_count(), 2);
    }

    #[test]
    fn repeated_cell_replaces_the_edit() {
        let coord = WorldChunkCoord::new(0, 0);
        let mut store = DeltaStore::new();
        store.set_height(coord, 7, 7, 30).expect("edit");
        store.set_height(coord, 7, 7, 50).expect("edit");
        assert_eq!(store.edit_count(), 1);
        assert_eq!(store.edits_for(coord), &[TerrainEdit::new(7, 7, 50)]);
    }

    #[test]
    fn height_at_uses_edits_and_generated_fallback() {
        let generator = WorldGenerator::new(42);
        let coord = WorldChunkCoord::new(0, 0);
        let pos = TerrainPos::new(10, 20);
        let untouched = generator.height_at(pos);
        let mut store = DeltaStore::new();
        store.set_height(coord, 10, 20, 77).expect("edit");
        assert_eq!(store.height_at(&generator, pos), 77);
        assert_eq!(
            store.height_at(&generator, TerrainPos::new(11, 20)),
            generator.height_at(TerrainPos::new(11, 20))
        );
        assert_ne!(untouched, 77);
    }

    #[test]
    fn checksum_changes_with_edits() {
        let mut store = DeltaStore::new();
        let empty = store.checksum();
        store
            .set_height(WorldChunkCoord::new(1, -1), 2, 3, 33)
            .expect("edit");
        assert_ne!(store.checksum(), empty);
        let copy = DeltaStore::decode(&store.encode()).expect("decode");
        assert_eq!(copy.checksum(), store.checksum());
    }

    #[test]
    fn rejects_heights_above_the_limit() {
        let mut store = DeltaStore::new();
        assert_eq!(
            store.set_height(WorldChunkCoord::new(0, 0), 0, 0, 256),
            Err(WorldError::InvalidHeight { height: 256 })
        );
    }

    #[test]
    fn binary_round_trip_preserves_negative_chunks() {
        let mut store = DeltaStore::new();
        store
            .set_height(WorldChunkCoord::new(-2, 3), 10, 20, 42)
            .expect("edit");
        store
            .set_height(WorldChunkCoord::new(0, 0), 255, 0, 90)
            .expect("edit");
        let encoded = store.encode();
        let decoded = DeltaStore::decode(&encoded).expect("decode");
        assert_eq!(decoded, store);
    }

    #[test]
    fn empty_store_round_trips() {
        let store = DeltaStore::new();
        let decoded = DeltaStore::decode(&store.encode()).expect("decode");
        assert_eq!(decoded, store);
        assert!(decoded.is_empty());
    }

    #[test]
    fn decode_bounds_edit_count_before_allocating() {
        let mut bytes = Vec::from(*b"TSDL");
        bytes.extend_from_slice(&DELTA_VERSION.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0i64.to_le_bytes());
        bytes.extend_from_slice(&0i64.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(DeltaStore::decode(&bytes), Err(WorldError::TruncatedDelta));
    }

    #[test]
    fn decode_rejects_out_of_range_heights() {
        let mut store = DeltaStore::new();
        store
            .set_height(WorldChunkCoord::new(0, 0), 1, 2, 42)
            .expect("edit");
        let mut bytes = store.encode();
        let len = bytes.len();
        bytes[len - 2..].copy_from_slice(&256u16.to_le_bytes());
        assert_eq!(
            DeltaStore::decode(&bytes),
            Err(WorldError::InvalidHeight { height: 256 })
        );
    }

    #[test]
    fn malformed_streams_are_rejected() {
        assert_eq!(
            DeltaStore::decode(b"nope"),
            Err(WorldError::InvalidDeltaMagic)
        );
        let mut bad_version = Vec::from(*b"TSDL");
        bad_version.extend_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            DeltaStore::decode(&bad_version),
            Err(WorldError::UnsupportedDeltaVersion { version: 2 })
        );
        let mut truncated = Vec::from(*b"TSDL");
        truncated.extend_from_slice(&DELTA_VERSION.to_le_bytes());
        assert_eq!(
            DeltaStore::decode(&truncated),
            Err(WorldError::TruncatedDelta)
        );
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut encoded = DeltaStore::new().encode();
        encoded.push(0);
        assert_eq!(
            DeltaStore::decode(&encoded),
            Err(WorldError::TrailingDeltaBytes { remaining: 1 })
        );
    }

    #[test]
    fn decode_rejects_duplicate_chunk_coordinates() {
        let mut bytes = Vec::from(*b"TSDL");
        bytes.extend_from_slice(&DELTA_VERSION.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for _ in 0..2 {
            bytes.extend_from_slice(&5i64.to_le_bytes());
            bytes.extend_from_slice(&(-6i64).to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        let error = DeltaStore::decode(&bytes).expect_err("duplicate chunk");
        assert_eq!(error, WorldError::DuplicateDeltaChunk { x: 5, z: -6 });
        assert_eq!(error.to_string(), "delta stream repeats chunk (5, -6)");
    }

    #[test]
    fn decode_rejects_duplicate_cells_in_a_chunk() {
        let mut bytes = Vec::from(*b"TSDL");
        bytes.extend_from_slice(&DELTA_VERSION.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&2i64.to_le_bytes());
        bytes.extend_from_slice(&3i64.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for _ in 0..2 {
            bytes.push(9);
            bytes.push(4);
            bytes.extend_from_slice(&40u16.to_le_bytes());
        }
        let error = DeltaStore::decode(&bytes).expect_err("duplicate cell");
        assert_eq!(
            error,
            WorldError::DuplicateDeltaEdit {
                chunk_x: 2,
                chunk_z: 3,
                x: 9,
                z: 4,
            }
        );
        assert_eq!(
            error.to_string(),
            "delta stream repeats cell (9, 4) in chunk (2, 3)"
        );
    }

    #[test]
    fn valid_blobs_re_encode_byte_identically() {
        let mut store = DeltaStore::new();
        let first = WorldChunkCoord::new(4, -3);
        let second = WorldChunkCoord::new(-1, 2);
        // Deliberately non-sorted insertion order: encoding preserves it.
        store.set_height(first, 200, 1, 50).expect("edit");
        store.set_height(first, 1, 200, 60).expect("edit");
        store.set_height(second, 3, 3, 70).expect("edit");
        store.set_height(first, 200, 1, 80).expect("edit");
        let encoded = store.encode();
        let decoded = DeltaStore::decode(&encoded).expect("decode");
        assert_eq!(decoded, store);
        assert_eq!(decoded.encode(), encoded);
        assert_eq!(decoded.checksum(), DeltaStore::checksum_bytes(&encoded));
    }

    #[test]
    fn full_chunk_terraform_reads_every_cell() {
        let coord = WorldChunkCoord::new(-1, 2);
        let generator = WorldGenerator::new(3);
        let mut store = DeltaStore::new();
        for z in 0..=u8::MAX {
            for x in 0..=u8::MAX {
                let height = (u16::from(x).wrapping_mul(7) + u16::from(z) * 13) % 256;
                store.set_height(coord, x, z, height).expect("edit");
            }
        }
        assert_eq!(store.edit_count(), WORLD_CHUNK_CELLS);
        assert_eq!(store.chunk_count(), 1);

        let origin = coord.origin_m();
        for z in 0..=u8::MAX {
            for x in 0..=u8::MAX {
                let expected = (u16::from(x).wrapping_mul(7) + u16::from(z) * 13) % 256;
                let pos = TerrainPos::new(origin.x + i64::from(x), origin.z + i64::from(z));
                assert_eq!(store.height_at(&generator, pos), expected);
            }
        }
    }

    #[test]
    fn encoding_capacity_matches_the_layout_not_the_old_hint() {
        let mut store = DeltaStore::new();
        let coord = WorldChunkCoord::new(0, 0);
        for index in 0..10_000u32 {
            let x = (index % 256) as u8;
            let z = (index / 256) as u8;
            store.set_height(coord, x, z, 40).expect("edit");
        }
        let encoded = store.encode();
        assert_eq!(store.encoded_len(), 10 + 20 + 4 * 10_000);
        assert_eq!(store.encoded_len(), encoded.len());
        assert!(encoded.capacity() >= encoded.len());
        // The old hint was `10 + edit_count * 20`; the layout is 4 bytes/edit.
        assert!(encoded.capacity() < 10 + 10_000 * 20);
    }
}
