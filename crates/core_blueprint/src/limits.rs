//! Package size and shape limits.

/// Maximum metadata JSON size (1 MiB).
pub const MAX_METADATA_BYTES: usize = 1024 * 1024;

/// Maximum voxel stream size (64 MiB).
pub const MAX_VOXEL_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// Maximum non-empty chunks in one package.
pub const MAX_CHUNKS: usize = core_voxel::volume::MAX_CHUNKS;

/// Maximum solid voxels in one package (8M).
pub const MAX_SOLID_VOXELS: u64 = 8_000_000;

/// Maximum functional props in one package.
pub const MAX_PROPS: usize = 10_000;
