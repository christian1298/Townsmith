//! Terrain biomes.
//!
//! The launch target is a temperate river valley with 3–4 biomes. This is a
//! deliberately small set for the Phase 0 prototype; Q-072 covers moddable
//! worldgen later.

/// A terrain biome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Biome {
    /// Open grassland.
    Plains = 0,
    /// Dense trees.
    Forest = 1,
    /// High, rocky terrain.
    Hills = 2,
    /// Land next to water.
    Coast = 3,
    /// A carved river valley.
    RiverValley = 4,
}

impl Biome {
    /// All biomes, in stable order.
    pub const ALL: [Self; 5] = [
        Self::Plains,
        Self::Forest,
        Self::Hills,
        Self::Coast,
        Self::RiverValley,
    ];

    /// Convert from the stable byte representation used by chunks.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Plains),
            1 => Some(Self::Forest),
            2 => Some(Self::Hills),
            3 => Some(Self::Coast),
            4 => Some(Self::RiverValley),
            _ => None,
        }
    }

    /// Stable byte representation.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_round_trip() {
        for biome in Biome::ALL {
            assert_eq!(Biome::from_u8(biome.as_u8()), Some(biome));
        }
        assert_eq!(Biome::from_u8(255), None);
    }
}
