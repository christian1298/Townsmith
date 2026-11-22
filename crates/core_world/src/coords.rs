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
    ///
    /// Saturates at the `i64` limits instead of overflowing. A chunk whose
    /// origin does not fit is not addressable by any [`TerrainPos`]; use
    /// [`Self::checked_origin_m`] to detect that case.
    #[must_use]
    pub const fn origin_m(self) -> TerrainPos {
        TerrainPos {
            x: self.x.saturating_mul(WORLD_CHUNK_METERS),
            z: self.z.saturating_mul(WORLD_CHUNK_METERS),
        }
    }

    /// The world-space origin of this chunk, or `None` when it does not fit in `i64`.
    #[must_use]
    pub const fn checked_origin_m(self) -> Option<TerrainPos> {
        match (
            self.x.checked_mul(WORLD_CHUNK_METERS),
            self.z.checked_mul(WORLD_CHUNK_METERS),
        ) {
            (Some(x), Some(z)) => Some(TerrainPos { x, z }),
            _ => None,
        }
    }

    /// Convert a world position to a local cell coordinate, if it belongs to this chunk.
    ///
    /// Uses floor division and remainder rather than subtracting a multiplied
    /// origin, so it is exact at `i64::MIN`/`i64::MAX` and never overflows.
    #[must_use]
    pub const fn local_from_world(self, pos: TerrainPos) -> Option<(usize, usize)> {
        if div_floor(pos.x, WORLD_CHUNK_METERS) != self.x
            || div_floor(pos.z, WORLD_CHUNK_METERS) != self.z
        {
            return None;
        }
        Some((
            rem_floor(pos.x, WORLD_CHUNK_METERS) as usize,
            rem_floor(pos.z, WORLD_CHUNK_METERS) as usize,
        ))
    }

    /// Chebyshev distance in chunks.
    ///
    /// Saturates at `i64::MAX` for coordinate pairs at opposite `i64` limits.
    #[must_use]
    pub const fn distance_chebyshev(self, other: Self) -> i64 {
        let dx = if self.x >= other.x {
            self.x.wrapping_sub(other.x) as u64
        } else {
            other.x.wrapping_sub(self.x) as u64
        };
        let dz = if self.z >= other.z {
            self.z.wrapping_sub(other.z) as u64
        } else {
            other.z.wrapping_sub(self.z) as u64
        };
        let distance = if dx > dz { dx } else { dz };
        if distance > i64::MAX as u64 {
            i64::MAX
        } else {
            distance as i64
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

    #[test]
    fn extreme_coordinates_do_not_overflow() {
        let max = TerrainPos::new(i64::MAX, i64::MIN);
        let max_chunk = WorldChunkCoord::from_terrain_pos(max);
        assert_eq!(
            max_chunk,
            WorldChunkCoord::new(36_028_797_018_963_967, -36_028_797_018_963_968)
        );
        assert_eq!(
            max_chunk.origin_m(),
            TerrainPos::new(i64::MAX - 255, i64::MIN)
        );
        assert_eq!(max_chunk.local_from_world(max), Some((255, 0)));
        assert_eq!(
            max_chunk.local_from_world(TerrainPos::new(i64::MIN, i64::MAX)),
            None
        );

        let saturated = WorldChunkCoord::new(i64::MAX, i64::MAX);
        assert_eq!(saturated.origin_m(), TerrainPos::new(i64::MAX, i64::MAX));
        assert_eq!(saturated.checked_origin_m(), None);
        assert_eq!(saturated.local_from_world(max), None);

        let min = TerrainPos::new(i64::MIN, i64::MIN);
        let min_chunk = WorldChunkCoord::from_terrain_pos(min);
        assert_eq!(
            min_chunk,
            WorldChunkCoord::new(i64::MIN / 256, i64::MIN / 256)
        );
        assert_eq!(min_chunk.origin_m(), min);
        assert_eq!(min_chunk.checked_origin_m(), Some(min));
        assert_eq!(min_chunk.local_from_world(min), Some((0, 0)));

        assert_eq!(
            WorldChunkCoord::new(i64::MIN, i64::MIN)
                .distance_chebyshev(WorldChunkCoord::new(i64::MAX, i64::MAX)),
            i64::MAX
        );
    }
}
