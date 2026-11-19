//! World and terrain coordinates.
//!
//! Terrain coordinates are integer meters. Chunk coordinates use floor
//! division so negative world positions map to the correct chunk.

/// Edge length of a world chunk in meters and in 1 m terrain cells.
pub const WORLD_CHUNK_SIZE: usize = 256;

/// Number of 1 m cells in one world chunk.
pub const WORLD_CHUNK_CELLS: usize = WORLD_CHUNK_SIZE * WORLD_CHUNK_SIZE;

/// Edge length of a world chunk in meters, as an integer.
pub const WORLD_CHUNK_METERS: i64 = WORLD_CHUNK_SIZE as i64;

/// Maximum terrain height in meters.
pub const MAX_HEIGHT_M: u16 = 255;

/// Sea level in meters.
pub const WATER_LEVEL_M: u16 = 24;

/// Floor division for signed integers.
#[must_use]
pub const fn div_floor(value: i64, divisor: i64) -> i64 {
    let quotient = value / divisor;
    let remainder = value % divisor;
    if remainder != 0 && ((remainder < 0) != (divisor < 0)) {
        quotient - 1
    } else {
        quotient
    }
}

/// Remainder that follows floor division; always non-negative for positive divisors.
#[must_use]
pub const fn rem_floor(value: i64, divisor: i64) -> i64 {
    value - div_floor(value, divisor) * divisor
}

/// A position on the 1 m terrain grid, in meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TerrainPos {
    /// East/west coordinate in meters.
    pub x: i64,
    /// North/south coordinate in meters.
    pub z: i64,
}

impl TerrainPos {
    /// Create a terrain position.
    #[must_use]
    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }
}

/// The coordinate of a 256 m world chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorldChunkCoord {
    /// Chunk coordinate on the X axis.
    pub x: i64,
    /// Chunk coordinate on the Z axis.
    pub z: i64,
}

impl WorldChunkCoord {
    /// Create a chunk coordinate.
    #[must_use]
    pub const fn new(x: i64, z: i64) -> Self {
        Self { x, z }
    }

    /// The chunk containing a terrain position.
    #[must_use]
    pub const fn from_terrain_pos(pos: TerrainPos) -> Self {
        Self {
            x: div_floor(pos.x, WORLD_CHUNK_METERS),
            z: div_floor(pos.z, WORLD_CHUNK_METERS),
        }
    }

    /// The world-space origin of this chunk, in meters.
    #[must_use]
    pub const fn origin_m(self) -> TerrainPos {
        TerrainPos {
            x: self.x * WORLD_CHUNK_METERS,
            z: self.z * WORLD_CHUNK_METERS,
        }
    }

    /// Convert a world position to a local cell coordinate, if it belongs to this chunk.
    #[must_use]
    pub const fn local_from_world(self, pos: TerrainPos) -> Option<(usize, usize)> {
        let dx = pos.x - self.origin_m().x;
        let dz = pos.z - self.origin_m().z;
        if dx < 0 || dz < 0 || dx >= WORLD_CHUNK_METERS || dz >= WORLD_CHUNK_METERS {
            return None;
        }
        Some((dx as usize, dz as usize))
    }

    /// Chebyshev distance in chunks.
    #[must_use]
    pub const fn distance_chebyshev(self, other: Self) -> i64 {
        let dx = if self.x > other.x {
            self.x - other.x
        } else {
            other.x - self.x
        };
        let dz = if self.z > other.z {
            self.z - other.z
        } else {
            other.z - self.z
        };
        if dx > dz {
            dx
        } else {
            dz
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_division_handles_negative_coordinates() {
        assert_eq!(div_floor(0, 256), 0);
        assert_eq!(div_floor(255, 256), 0);
        assert_eq!(div_floor(256, 256), 1);
        assert_eq!(div_floor(-1, 256), -1);
        assert_eq!(div_floor(-256, 256), -1);
        assert_eq!(div_floor(-257, 256), -2);
        assert_eq!(rem_floor(-1, 256), 255);
    }

    #[test]
    fn negative_positions_map_to_the_correct_chunk() {
        let coord = WorldChunkCoord::from_terrain_pos(TerrainPos::new(-1, -257));
        assert_eq!(coord, WorldChunkCoord::new(-1, -2));
        assert_eq!(
            coord.local_from_world(TerrainPos::new(-1, -257)),
            Some((255, 255))
        );
    }

    #[test]
    fn origin_and_local_coordinates_round_trip() {
        let coord = WorldChunkCoord::new(3, -4);
        let origin = coord.origin_m();
        assert_eq!(origin, TerrainPos::new(768, -1024));
        assert_eq!(coord.local_from_world(origin), Some((0, 0)));
        assert_eq!(
            coord.local_from_world(TerrainPos::new(768 + 255, -1024 + 255)),
            Some((255, 255))
        );
        assert_eq!(coord.local_from_world(TerrainPos::new(1024, -1024)), None);
    }

    #[test]
    fn chebyshev_distance_is_symmetric() {
        let a = WorldChunkCoord::new(0, 0);
        let b = WorldChunkCoord::new(-3, 2);
        assert_eq!(a.distance_chebyshev(b), 3);
        assert_eq!(b.distance_chebyshev(a), 3);
    }
}
