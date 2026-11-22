//! Errors returned by world generation, chunks, deltas, and streaming.

use std::error::Error;
use std::fmt;

/// A world, chunk, delta, or streaming error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldError {
    /// A local cell coordinate was outside the 256 x 256 chunk.
    OutOfBounds { x: usize, z: usize },
    /// A chunk was built with the wrong number of cells.
    InvalidChunkLength { expected: usize, actual: usize },
    /// A byte stream is not a Townsmith delta file.
    InvalidDeltaMagic,
    /// The delta file version is not supported.
    UnsupportedDeltaVersion { version: u16 },
    /// The delta byte stream ended in the middle of a record.
    TruncatedDelta,
    /// A delta contained a biome byte outside the known set.
    InvalidBiome { value: u8 },
    /// A terrain height exceeded the 0..=255 range.
    InvalidHeight { height: u16 },
    /// Bytes remained after the last delta record.
    TrailingDeltaBytes { remaining: usize },
    /// Two delta records addressed the same chunk.
    DuplicateDeltaChunk { x: i64, z: i64 },
    /// Two edits in one chunk addressed the same local cell.
    DuplicateDeltaEdit {
        chunk_x: i64,
        chunk_z: i64,
        x: u8,
        z: u8,
    },
}

impl fmt::Display for WorldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds { x, z } => {
                write!(f, "terrain cell ({x}, {z}) is outside the world chunk")
            }
            Self::InvalidChunkLength { expected, actual } => {
                write!(f, "world chunk has {actual} cells, expected {expected}")
            }
            Self::InvalidDeltaMagic => write!(f, "delta stream has an invalid magic header"),
            Self::UnsupportedDeltaVersion { version } => {
                write!(f, "delta stream version {version} is not supported")
            }
            Self::TruncatedDelta => write!(f, "delta stream is truncated"),
            Self::InvalidBiome { value } => write!(f, "unknown biome byte {value}"),
            Self::InvalidHeight { height } => {
                write!(f, "terrain height {height} exceeds the 255 m limit")
            }
            Self::TrailingDeltaBytes { remaining } => {
                write!(f, "delta stream has {remaining} trailing bytes")
            }
            Self::DuplicateDeltaChunk { x, z } => {
                write!(f, "delta stream repeats chunk ({x}, {z})")
            }
            Self::DuplicateDeltaEdit {
                chunk_x,
                chunk_z,
                x,
                z,
            } => {
                write!(
                    f,
                    "delta stream repeats cell ({x}, {z}) in chunk ({chunk_x}, {chunk_z})"
                )
            }
        }
    }
}

impl Error for WorldError {}
