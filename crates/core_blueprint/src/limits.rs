//! Package size and shape limits.

/// Maximum metadata JSON size (1 MiB).
pub const MAX_METADATA_BYTES: usize = 1024 * 1024;

/// Maximum voxel stream size (64 MiB).
pub const MAX_VOXEL_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// Maximum non-empty chunks in one package.
pub const MAX_CHUNKS: usize = core_voxel::volume::MAX_CHUNKS;

/// Maximum solid voxels in one package (8M).
pub const MAX_SOLID_VOXELS: u64 = 8_000_000;

/// Maximum voxels the declared volume grid may allocate, including air.
///
/// A package may not declare a chunk grid whose dense capacity exceeds the
/// solid-voxel budget: 8M voxels is 244 chunks of 32³, so an untrusted
/// `voxels.bin` header can force at most ~16 MiB of chunk storage before the
/// stream is rejected. This is a decode-safety bound, not a design ceiling;
/// larger buildings are composed from multiple blueprints.
pub const MAX_VOLUME_VOXELS: u64 = MAX_SOLID_VOXELS;

/// Maximum functional props in one package.
pub const MAX_PROPS: usize = 10_000;
