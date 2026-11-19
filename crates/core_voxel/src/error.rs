//! Errors returned by voxel and palette operations.

use std::error::Error;
use std::fmt;

/// A voxel, palette, or encoding error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoxelError {
    /// A coordinate was outside the target chunk or volume.
    OutOfBounds { x: usize, y: usize, z: usize },
    /// A run-length stream did not cover exactly one chunk.
    InvalidRunLength { expected: usize, actual: usize },
    /// A palette index did not exist.
    InvalidPaletteIndex { index: u16, palette_len: usize },
    /// A palette was empty.
    EmptyPalette,
    /// The first palette entry must be air.
    PaletteMustStartWithAir,
    /// A run had a zero length.
    ZeroLengthRun,
}

impl fmt::Display for VoxelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { x, y, z } => {
                write!(
                    f,
                    "voxel coordinate ({x}, {y}, {z}) is outside the target grid"
                )
            }
            Self::InvalidRunLength { expected, actual } => {
                write!(f, "RLE stream covers {actual} voxels, expected {expected}")
            }
            Self::InvalidPaletteIndex { index, palette_len } => {
                write!(
                    f,
                    "palette index {index} is outside palette length {palette_len}"
                )
            }
            Self::EmptyPalette => write!(f, "palette must contain at least air"),
            Self::PaletteMustStartWithAir => write!(f, "palette entry 0 must be air"),
            Self::ZeroLengthRun => write!(f, "RLE run must not have zero length"),
        }
    }
}

impl Error for VoxelError {}
