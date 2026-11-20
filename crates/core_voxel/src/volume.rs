//! A chunked, editable voxel volume larger than one 32³ chunk.
//!
//! A volume is a box of chunks addressed in 10 cm voxel coordinates. Chunks are
//! allocated on first write, so an empty 16 m × 9.6 m × 16 m volume costs only
//! the chunk pointer table. [`VoxelVolume::mesh`] meshes the whole volume at
//! once: faces between adjacent solid voxels are culled across chunk
//! boundaries, and coplanar greedy rectangles may span chunks.

use std::fmt;

use crate::chunk::VoxelChunk;
use crate::coords::{MaterialId, AIR, CHUNK_SIZE, VOXEL_SIZE_M};
use crate::error::VoxelError;
use crate::mesh::{greedy_mesh_grid, MeshBuffers};

/// Maximum number of chunks on one volume axis.
///
/// At 32 voxels per chunk this is 102.4 m, comfortably beyond the 48 m
/// blueprint footprint target.
pub const MAX_CHUNKS_PER_AXIS: u32 = 32;

/// Maximum total chunks in one volume.
///
/// Chunks are allocated lazily. At the cap a fully solid volume would hold
/// 4096 × 32,768 = 134,217,728 voxels (256 MiB at two bytes per voxel).
pub const MAX_CHUNKS: usize = 4096;

/// Default chunks on the X axis.
pub const DEFAULT_CHUNKS_X: u32 = 5;
/// Default chunks on the Y (vertical) axis.
pub const DEFAULT_CHUNKS_Y: u32 = 3;
/// Default chunks on the Z axis.
pub const DEFAULT_CHUNKS_Z: u32 = 5;

const CHUNK_SIZE_U32: u32 = CHUNK_SIZE as u32;

/// Chunk-grid dimensions of a [`VoxelVolume`].
///
/// Use [`VolumeConfig::new`] or [`VolumeConfig::validate`] before handing a
/// config to [`VoxelVolume::new`]. The default is 5 × 3 × 5 chunks:
/// 16 m × 9.6 m × 16 m, enough for the 16 m × 8 m × 16 m vertical-slice house.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeConfig {
    /// Number of chunks along X.
    pub chunks_x: u32,
    /// Number of chunks along Y (vertical).
    pub chunks_y: u32,
    /// Number of chunks along Z.
    pub chunks_z: u32,
}

impl VolumeConfig {
    /// Create and validate a volume config.
    pub fn new(chunks_x: u32, chunks_y: u32, chunks_z: u32) -> Result<Self, VolumeConfigError> {
        let config = Self {
            chunks_x,
            chunks_y,
            chunks_z,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate axis counts and the total chunk budget.
    pub fn validate(self) -> Result<(), VolumeConfigError> {
        for (axis, chunks) in [self.chunks_x, self.chunks_y, self.chunks_z]
            .into_iter()
            .enumerate()
        {
            if chunks == 0 {
                return Err(VolumeConfigError::EmptyAxis { axis });
            }
            if chunks > MAX_CHUNKS_PER_AXIS {
                return Err(VolumeConfigError::AxisTooLarge {
                    axis,
                    chunks,
                    max: MAX_CHUNKS_PER_AXIS,
                });
            }
        }

        let chunks = self.chunk_count();
        if chunks > MAX_CHUNKS {
            return Err(VolumeConfigError::TooManyChunks {
                chunks,
                max: MAX_CHUNKS,
            });
        }
        Ok(())
    }

    /// Chunk counts as `[x, y, z]`.
    #[must_use]
    pub const fn chunks(self) -> [u32; 3] {
        [self.chunks_x, self.chunks_y, self.chunks_z]
    }

    /// Total chunks in the volume, saturating on absurd unvalidated configs.
    #[must_use]
    pub fn chunk_count(self) -> usize {
        let total =
            u128::from(self.chunks_x) * u128::from(self.chunks_y) * u128::from(self.chunks_z);
        usize::try_from(total).unwrap_or(usize::MAX)
    }

    /// Voxel dimensions as `[x, y, z]`, saturating on absurd unvalidated configs.
    #[must_use]
    pub fn voxels(self) -> [usize; 3] {
        [self.chunks_x, self.chunks_y, self.chunks_z].map(|chunks| {
            usize::try_from(u128::from(chunks) * u128::from(CHUNK_SIZE_U32)).unwrap_or(usize::MAX)
        })
    }

    /// Volume bounds in meters as `[x, y, z]`.
    #[must_use]
    pub fn bounds_m(self) -> [f32; 3] {
        self.voxels().map(|voxels| voxels as f32 * VOXEL_SIZE_M)
    }
}

impl Default for VolumeConfig {
    fn default() -> Self {
        Self {
            chunks_x: DEFAULT_CHUNKS_X,
            chunks_y: DEFAULT_CHUNKS_Y,
            chunks_z: DEFAULT_CHUNKS_Z,
        }
    }
}

/// Why a [`VolumeConfig`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeConfigError {
    /// An axis had zero chunks.
    EmptyAxis { axis: usize },
    /// An axis exceeded [`MAX_CHUNKS_PER_AXIS`].
    AxisTooLarge { axis: usize, chunks: u32, max: u32 },
    /// The total chunk count exceeded [`MAX_CHUNKS`].
    TooManyChunks { chunks: usize, max: usize },
}

impl fmt::Display for VolumeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyAxis { axis } => {
                write!(f, "volume axis {axis} must contain at least one chunk")
            }
            Self::AxisTooLarge { axis, chunks, max } => write!(
                f,
                "volume axis {axis} has {chunks} chunks, the maximum is {max}"
            ),
            Self::TooManyChunks { chunks, max } => {
                write!(f, "volume has {chunks} chunks, the maximum is {max}")
            }
        }
    }
}

impl std::error::Error for VolumeConfigError {}

/// A chunk coordinate inside a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkPos {
    /// Chunk index along X.
    pub x: u32,
    /// Chunk index along Y.
    pub y: u32,
    /// Chunk index along Z.
    pub z: u32,
}

impl ChunkPos {
    /// Create a chunk coordinate.
    #[must_use]
    pub const fn new(x: u32, y: u32, z: u32) -> Self {
        Self { x, y, z }
    }

    /// First voxel of this chunk in volume voxel coordinates.
    #[must_use]
    pub const fn origin_voxels(self) -> [u32; 3] {
        [
            self.x.saturating_mul(CHUNK_SIZE_U32),
            self.y.saturating_mul(CHUNK_SIZE_U32),
            self.z.saturating_mul(CHUNK_SIZE_U32),
        ]
    }

    /// First voxel of this chunk in volume meters.
    #[must_use]
    pub fn origin_meters(self) -> [f32; 3] {
        self.origin_voxels()
            .map(|voxel| voxel as f32 * VOXEL_SIZE_M)
    }
}

/// A voxel coordinate in a volume, in 10 cm voxels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VolumePos {
    /// Voxel index along X.
    pub x: u32,
    /// Voxel index along Y (vertical).
    pub y: u32,
    /// Voxel index along Z.
    pub z: u32,
}

impl VolumePos {
    /// Create a volume voxel coordinate.
    #[must_use]
    pub const fn new(x: u32, y: u32, z: u32) -> Self {
        Self { x, y, z }
    }

    /// Chunk that contains this voxel.
    #[must_use]
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.x / CHUNK_SIZE_U32,
            y: self.y / CHUNK_SIZE_U32,
            z: self.z / CHUNK_SIZE_U32,
        }
    }

    /// Chunk-local coordinate as `[x, y, z]`.
    #[must_use]
    pub const fn local(self) -> [usize; 3] {
        [
            (self.x % CHUNK_SIZE_U32) as usize,
            (self.y % CHUNK_SIZE_U32) as usize,
            (self.z % CHUNK_SIZE_U32) as usize,
        ]
    }
}

/// A box of chunks with lazy per-chunk allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoxelVolume {
    config: VolumeConfig,
    chunks: Vec<Option<VoxelChunk>>,
}

impl VoxelVolume {
    /// Create an empty volume.
    pub fn new(config: VolumeConfig) -> Result<Self, VolumeConfigError> {
        config.validate()?;
        Ok(Self {
            chunks: vec![None; config.chunk_count()],
            config,
        })
    }

    /// The volume configuration.
    #[must_use]
    pub fn config(&self) -> VolumeConfig {
        self.config
    }

    /// Whether a voxel coordinate is inside the volume.
    #[must_use]
    pub fn contains(&self, pos: VolumePos) -> bool {
        let dims = self.config.voxels();
        (pos.x as usize) < dims[0] && (pos.y as usize) < dims[1] && (pos.z as usize) < dims[2]
    }

    /// Read a voxel, returning a typed error outside the volume.
    pub fn get_voxel(&self, pos: VolumePos) -> Result<MaterialId, VoxelError> {
        if !self.contains(pos) {
            return Err(oob(pos));
        }
        Ok(self.material_at_pos(pos))
    }

    /// Write a voxel, returning a typed error outside the volume.
    ///
    /// Writing [`AIR`] to an unallocated chunk does not allocate it. Writing
    /// air to an allocated chunk keeps the (now empty) chunk allocated until
    /// [`Self::clear_chunk`] or [`Self::clear`] is called.
    pub fn set_voxel(&mut self, pos: VolumePos, material: MaterialId) -> Result<(), VoxelError> {
        if !self.contains(pos) {
            return Err(oob(pos));
        }

        let chunk_pos = pos.chunk();
        let index = self.chunk_index(chunk_pos).ok_or_else(|| oob(pos))?;
        let local = pos.local();

        if material == AIR {
            if let Some(chunk) = self.chunks[index].as_mut() {
                chunk.set(local[0], local[1], local[2], AIR)?;
            }
            return Ok(());
        }

        let chunk = self.chunks[index].get_or_insert_with(VoxelChunk::new);
        chunk.set(local[0], local[1], local[2], material)
    }

    /// Fill a half-open box `[min, max)` with a material.
    ///
    /// The whole box is validated before anything is written, so a rejected
    /// call leaves the volume unchanged.
    pub fn fill_box(
        &mut self,
        min: VolumePos,
        max: VolumePos,
        material: MaterialId,
    ) -> Result<(), VoxelError> {
        self.validate_box(min, max)?;

        if min.x == max.x || min.y == max.y || min.z == max.z {
            return Ok(());
        }

        let start = min.chunk();
        let last = VolumePos::new(max.x - 1, max.y - 1, max.z - 1).chunk();

        for chunk_z in start.z..=last.z {
            for chunk_y in start.y..=last.y {
                for chunk_x in start.x..=last.x {
                    let chunk_pos = ChunkPos::new(chunk_x, chunk_y, chunk_z);
                    let index = self
                        .chunk_index(chunk_pos)
                        .expect("validated box maps to allocated chunk slots");

                    if material == AIR && self.chunks[index].is_none() {
                        continue;
                    }

                    let origin = chunk_pos.origin_voxels();
                    let local_min = [
                        (min.x.max(origin[0]) - origin[0]) as usize,
                        (min.y.max(origin[1]) - origin[1]) as usize,
                        (min.z.max(origin[2]) - origin[2]) as usize,
                    ];
                    let local_max = [
                        (max.x.min(origin[0].saturating_add(CHUNK_SIZE_U32)) - origin[0]) as usize,
                        (max.y.min(origin[1].saturating_add(CHUNK_SIZE_U32)) - origin[1]) as usize,
                        (max.z.min(origin[2].saturating_add(CHUNK_SIZE_U32)) - origin[2]) as usize,
                    ];

                    let chunk = self.chunks[index].get_or_insert_with(VoxelChunk::new);
                    chunk.fill_box(local_min, local_max, material)?;
                }
            }
        }
        Ok(())
    }

    /// Number of non-air voxels across allocated chunks.
    #[must_use]
    pub fn solid_count(&self) -> u32 {
        self.chunks
            .iter()
            .flatten()
            .map(VoxelChunk::solid_count)
            .sum()
    }

    /// Number of chunks currently allocated.
    #[must_use]
    pub fn chunks_allocated(&self) -> usize {
        self.chunks.iter().filter(|chunk| chunk.is_some()).count()
    }

    /// Whether the volume contains only air.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.solid_count() == 0
    }

    /// Insert a fully decoded chunk at `chunk`.
    ///
    /// The chunk coordinate must be inside the configured volume. Existing
    /// content at that coordinate is replaced. Blueprint loading uses this to
    /// restore non-empty chunks without replaying every voxel.
    pub fn insert_chunk(&mut self, chunk: ChunkPos, data: VoxelChunk) -> Result<(), VoxelError> {
        let origin = chunk.origin_voxels();
        let index = self.chunk_index(chunk).ok_or(VoxelError::OutOfBounds {
            x: usize::try_from(origin[0]).unwrap_or(usize::MAX),
            y: usize::try_from(origin[1]).unwrap_or(usize::MAX),
            z: usize::try_from(origin[2]).unwrap_or(usize::MAX),
        })?;
        self.chunks[index] = Some(data);
        Ok(())
    }

    /// Read-only view of an allocated chunk.
    #[must_use]
    pub fn chunk_at(&self, chunk: ChunkPos) -> Option<&VoxelChunk> {
        self.chunk_index(chunk)
            .and_then(|index| self.chunks[index].as_ref())
    }

    /// Mutable view of an allocated chunk.
    #[must_use]
    pub fn chunk_at_mut(&mut self, chunk: ChunkPos) -> Option<&mut VoxelChunk> {
        self.chunk_index(chunk)
            .and_then(|index| self.chunks[index].as_mut())
    }

    /// Remove one chunk, returning it when it was allocated.
    pub fn clear_chunk(&mut self, chunk: ChunkPos) -> Option<VoxelChunk> {
        self.chunk_index(chunk)
            .and_then(|index| self.chunks[index].take())
    }

    /// Remove every chunk.
    pub fn clear(&mut self) {
        self.chunks.fill(None);
    }

    /// Mesh the whole volume into one engine-neutral buffer.
    ///
    /// The mesher walks chunks in deterministic X, then Y, then Z order, culls
    /// faces between adjacent solid voxels across chunk boundaries, and merges
    /// coplanar faces of the same material. Positions are in volume meters;
    /// UVs stay slice-local in meters.
    #[must_use]
    pub fn mesh(&self) -> MeshBuffers {
        if self.is_empty() {
            return MeshBuffers::default();
        }
        greedy_mesh_grid(self.config.voxels(), [0.0; 3], |coord| {
            if coord.iter().any(|&value| value < 0) {
                AIR
            } else {
                self.material_at([coord[0] as usize, coord[1] as usize, coord[2] as usize])
            }
        })
    }

    fn validate_box(&self, min: VolumePos, max: VolumePos) -> Result<(), VoxelError> {
        let dims = self.config.voxels();
        let lo = [min.x as usize, min.y as usize, min.z as usize];
        let hi = [max.x as usize, max.y as usize, max.z as usize];

        for axis in 0..3 {
            if lo[axis] > hi[axis] || hi[axis] > dims[axis] {
                let error = VolumePos {
                    x: if axis == 0 { max.x } else { min.x },
                    y: if axis == 1 { max.y } else { min.y },
                    z: if axis == 2 { max.z } else { min.z },
                };
                return Err(oob(error));
            }
        }
        Ok(())
    }

    fn chunk_index(&self, chunk: ChunkPos) -> Option<usize> {
        if chunk.x >= self.config.chunks_x
            || chunk.y >= self.config.chunks_y
            || chunk.z >= self.config.chunks_z
        {
            return None;
        }
        let chunks_x = self.config.chunks_x as usize;
        let chunks_y = self.config.chunks_y as usize;
        Some(
            chunk.x as usize + chunk.y as usize * chunks_x + chunk.z as usize * chunks_x * chunks_y,
        )
    }

    fn material_at_pos(&self, pos: VolumePos) -> MaterialId {
        self.chunk_at(pos.chunk()).map_or(AIR, |chunk| {
            let local = pos.local();
            chunk.get(local[0], local[1], local[2])
        })
    }

    fn material_at(&self, coord: [usize; 3]) -> MaterialId {
        let dims = self.config.voxels();
        if coord[0] >= dims[0] || coord[1] >= dims[1] || coord[2] >= dims[2] {
            return AIR;
        }

        let chunk_pos = ChunkPos::new(
            (coord[0] / CHUNK_SIZE) as u32,
            (coord[1] / CHUNK_SIZE) as u32,
            (coord[2] / CHUNK_SIZE) as u32,
        );
        self.chunk_at(chunk_pos).map_or(AIR, |chunk| {
            chunk.get(
                coord[0] % CHUNK_SIZE,
                coord[1] % CHUNK_SIZE,
                coord[2] % CHUNK_SIZE,
            )
        })
    }
}

impl Default for VoxelVolume {
    fn default() -> Self {
        Self::new(VolumeConfig::default()).expect("the default volume config is valid")
    }
}

fn oob(pos: VolumePos) -> VoxelError {
    VoxelError::OutOfBounds {
        x: pos.x as usize,
        y: pos.y as usize,
        z: pos.z as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(chunks_x: u32, chunks_y: u32, chunks_z: u32) -> VoxelVolume {
        let config = VolumeConfig::new(chunks_x, chunks_y, chunks_z).expect("valid config");
        VoxelVolume::new(config).expect("valid volume")
    }

    fn assert_indices_in_bounds(mesh: &MeshBuffers) {
        let vertex_count = mesh.vertices.len() as u32;
        for &index in &mesh.indices {
            assert!(index < vertex_count, "index {index} >= {vertex_count}");
        }
    }

    fn sample_volume() -> VoxelVolume {
        let mut volume = volume(2, 2, 2);
        volume
            .fill_box(VolumePos::new(1, 1, 1), VolumePos::new(20, 8, 20), 1)
            .unwrap();
        volume
            .fill_box(VolumePos::new(30, 30, 30), VolumePos::new(40, 36, 40), 2)
            .unwrap();
        volume
            .fill_box(VolumePos::new(5, 20, 5), VolumePos::new(10, 24, 10), 3)
            .unwrap();
        volume
    }

    #[test]
    fn default_config_is_the_vertical_slice_house() {
        let config = VolumeConfig::default();
        assert_eq!(config.chunks(), [5, 3, 5]);
        assert_eq!(config.chunk_count(), 75);
        assert_eq!(config.voxels(), [160, 96, 160]);
        config.validate().unwrap();

        let bounds = config.bounds_m();
        assert!((bounds[0] - 16.0).abs() < 1e-4);
        assert!((bounds[1] - 9.6).abs() < 1e-4);
        assert!((bounds[2] - 16.0).abs() < 1e-4);
    }

    #[test]
    fn config_rejects_empty_and_oversized_axes() {
        assert_eq!(
            VolumeConfig::new(0, 1, 1),
            Err(VolumeConfigError::EmptyAxis { axis: 0 })
        );
        assert_eq!(
            VolumeConfig::new(1, MAX_CHUNKS_PER_AXIS + 1, 1),
            Err(VolumeConfigError::AxisTooLarge {
                axis: 1,
                chunks: MAX_CHUNKS_PER_AXIS + 1,
                max: MAX_CHUNKS_PER_AXIS,
            })
        );
        assert_eq!(
            VolumeConfig::new(16, 16, 17),
            Err(VolumeConfigError::TooManyChunks {
                chunks: 4352,
                max: MAX_CHUNKS,
            })
        );
    }

    #[test]
    fn set_and_get_across_chunk_boundaries() {
        let mut volume = volume(2, 2, 2);
        volume.set_voxel(VolumePos::new(31, 31, 31), 1).unwrap();
        volume.set_voxel(VolumePos::new(32, 32, 32), 2).unwrap();

        assert_eq!(volume.get_voxel(VolumePos::new(31, 31, 31)), Ok(1));
        assert_eq!(volume.get_voxel(VolumePos::new(32, 32, 32)), Ok(2));
        assert_eq!(volume.get_voxel(VolumePos::new(0, 0, 0)), Ok(AIR));
        assert_eq!(volume.solid_count(), 2);
        assert!(volume.chunk_at(ChunkPos::new(0, 0, 0)).is_some());
        assert!(volume.chunk_at(ChunkPos::new(1, 1, 1)).is_some());
        assert!(volume.chunk_at(ChunkPos::new(2, 0, 0)).is_none());
    }

    #[test]
    fn out_of_bounds_reads_and_writes_are_typed_errors() {
        let mut volume = VoxelVolume::default();
        assert_eq!(
            volume.get_voxel(VolumePos::new(160, 0, 0)),
            Err(VoxelError::OutOfBounds { x: 160, y: 0, z: 0 })
        );
        assert_eq!(
            volume.set_voxel(VolumePos::new(0, 96, 0), 1),
            Err(VoxelError::OutOfBounds { x: 0, y: 96, z: 0 })
        );
        assert_eq!(volume.solid_count(), 0);
        assert_eq!(volume.chunks_allocated(), 0);
    }

    #[test]
    fn fill_box_spans_chunks() {
        let mut volume = volume(2, 1, 1);
        volume
            .fill_box(VolumePos::new(30, 0, 0), VolumePos::new(34, 2, 2), 9)
            .unwrap();

        assert_eq!(volume.solid_count(), 16);
        assert_eq!(volume.get_voxel(VolumePos::new(30, 0, 0)), Ok(9));
        assert_eq!(volume.get_voxel(VolumePos::new(33, 1, 1)), Ok(9));
        assert_eq!(volume.get_voxel(VolumePos::new(34, 1, 1)), Ok(AIR));
        assert_eq!(volume.chunks_allocated(), 2);
    }

    #[test]
    fn fill_box_rejects_invalid_bounds_without_partial_writes() {
        let mut volume = VoxelVolume::default();
        assert_eq!(
            volume.fill_box(VolumePos::new(0, 0, 0), VolumePos::new(161, 1, 1), 1),
            Err(VoxelError::OutOfBounds { x: 161, y: 0, z: 0 })
        );
        assert_eq!(volume.solid_count(), 0);
        assert_eq!(volume.chunks_allocated(), 0);

        assert_eq!(
            volume.fill_box(VolumePos::new(0, 0, 0), VolumePos::new(1, 1, 161), 1),
            Err(VoxelError::OutOfBounds { x: 0, y: 0, z: 161 })
        );
        assert_eq!(
            volume.fill_box(VolumePos::new(1, 0, 0), VolumePos::new(0, 1, 1), 1),
            Err(VoxelError::OutOfBounds { x: 0, y: 0, z: 0 })
        );
        assert_eq!(volume.solid_count(), 0);
    }

    #[test]
    fn filling_air_does_not_allocate_chunks() {
        let mut volume = VoxelVolume::default();
        volume
            .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(8, 8, 8), AIR)
            .unwrap();
        assert_eq!(volume.chunks_allocated(), 0);
        assert_eq!(volume.solid_count(), 0);
    }

    #[test]
    fn clear_chunk_and_clear_remove_chunks() {
        let mut volume = volume(2, 2, 2);
        volume
            .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(64, 64, 64), 1)
            .unwrap();
        assert_eq!(volume.chunks_allocated(), 8);
        assert_eq!(volume.solid_count(), 8 * 32 * 32 * 32);

        let before = volume.solid_count();
        let removed = volume
            .clear_chunk(ChunkPos::new(0, 0, 0))
            .expect("chunk is allocated");
        assert_eq!(volume.solid_count(), before - removed.solid_count());
        assert_eq!(volume.chunks_allocated(), 7);

        volume.clear();
        assert_eq!(volume.solid_count(), 0);
        assert_eq!(volume.chunks_allocated(), 0);
        assert!(volume.is_empty());
        assert!(volume.mesh().is_empty());
    }

    #[test]
    fn single_voxel_has_six_quads() {
        let mut volume = VoxelVolume::default();
        volume.set_voxel(VolumePos::new(10, 10, 10), 1).unwrap();
        let mesh = volume.mesh();
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_eq!(mesh.indices.len(), 36);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn adjacent_voxels_across_a_chunk_boundary_merge_to_six_quads() {
        let mut volume = volume(2, 1, 1);
        volume.set_voxel(VolumePos::new(31, 0, 0), 1).unwrap();
        volume.set_voxel(VolumePos::new(32, 0, 0), 1).unwrap();

        let mesh = volume.mesh();
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn solid_two_by_two_by_two_block_has_six_quads_across_chunks() {
        let mut volume = volume(2, 2, 2);
        volume
            .fill_box(VolumePos::new(31, 31, 31), VolumePos::new(33, 33, 33), 1)
            .unwrap();

        assert_eq!(volume.solid_count(), 8);
        let mesh = volume.mesh();
        assert_eq!(mesh.quad_count(), 6);
        assert_eq!(mesh.vertices.len(), 24);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn different_materials_are_not_merged() {
        let mut volume = VoxelVolume::default();
        volume.set_voxel(VolumePos::new(0, 0, 0), 1).unwrap();
        volume.set_voxel(VolumePos::new(1, 0, 0), 2).unwrap();

        let mesh = volume.mesh();
        assert_eq!(mesh.quad_count(), 10);
        assert_indices_in_bounds(&mesh);
    }

    #[test]
    fn insert_chunk_replaces_existing_chunk_and_rejects_out_of_bounds() {
        let mut volume = VoxelVolume::new(VolumeConfig::new(2, 1, 2).unwrap()).unwrap();
        let mut chunk = VoxelChunk::new();
        chunk.set(1, 2, 3, 7).unwrap();
        volume.insert_chunk(ChunkPos::new(1, 0, 1), chunk).unwrap();
        assert_eq!(volume.get_voxel(VolumePos::new(33, 2, 35)).unwrap(), 7);
        assert!(volume
            .insert_chunk(ChunkPos::new(2, 0, 0), VoxelChunk::new())
            .is_err());
    }

    #[test]
    fn mesh_positions_are_offset_to_volume_meters() {
        let mut volume = volume(2, 1, 1);
        volume.set_voxel(VolumePos::new(32, 0, 0), 1).unwrap();
        let mesh = volume.mesh();

        let min_x = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::NEG_INFINITY, f32::max);

        assert!((min_x - 3.2).abs() < 1e-4, "min_x = {min_x}");
        assert!((max_x - 3.3).abs() < 1e-4, "max_x = {max_x}");
        for vertex in &mesh.vertices {
            assert!((-1e-4..=0.1 + 1e-4).contains(&vertex.position[1]));
            assert!((-1e-4..=0.1 + 1e-4).contains(&vertex.position[2]));
        }
    }

    #[test]
    fn mesh_positions_stay_inside_volume_bounds_and_indices_are_valid() {
        let volume = sample_volume();
        let bounds = volume.config().bounds_m();
        let mesh = volume.mesh();

        assert!(!mesh.is_empty());
        assert_indices_in_bounds(&mesh);
        for vertex in &mesh.vertices {
            for (axis, bound) in bounds.iter().enumerate() {
                let position = vertex.position[axis];
                assert!(position >= -1e-4, "negative position {position}");
                assert!(
                    position <= *bound + 1e-4,
                    "position {position} outside bound {bound}"
                );
            }
        }
    }

    #[test]
    fn empty_volume_mesh_is_empty() {
        let volume = VoxelVolume::default();
        let mesh = volume.mesh();
        assert!(mesh.is_empty());
        assert_eq!(mesh.quad_count(), 0);
        assert_eq!(mesh.checksum(), MeshBuffers::default().checksum());
    }

    #[test]
    fn mesh_is_deterministic() {
        let first = sample_volume();
        let second = sample_volume();

        let first_mesh = first.mesh();
        let second_mesh = second.mesh();
        assert_eq!(first_mesh, second_mesh);
        assert_eq!(first_mesh.checksum(), second_mesh.checksum());
        assert_eq!(first_mesh, first.mesh());
    }

    #[test]
    fn fill_box_accepts_exclusive_max_at_volume_edge() {
        let mut volume = volume(1, 1, 1);
        volume
            .fill_box(VolumePos::new(0, 0, 0), VolumePos::new(32, 32, 32), 1)
            .unwrap();
        assert_eq!(volume.solid_count(), 32 * 32 * 32);
        assert_eq!(volume.chunks_allocated(), 1);
    }

    #[test]
    fn air_write_keeps_allocated_chunk() {
        let mut volume = volume(1, 1, 1);
        volume.set_voxel(VolumePos::new(1, 1, 1), 1).unwrap();
        assert_eq!(volume.chunks_allocated(), 1);
        volume.set_voxel(VolumePos::new(1, 1, 1), AIR).unwrap();
        assert_eq!(volume.chunks_allocated(), 1);
        assert_eq!(volume.solid_count(), 0);
    }

    #[test]
    fn config_at_total_chunk_cap_is_accepted() {
        let config = VolumeConfig::new(16, 16, 16).expect("4096 chunks is the cap");
        assert_eq!(config.chunk_count(), 4096);
    }

    #[test]
    fn config_above_total_chunk_cap_is_rejected() {
        assert!(matches!(
            VolumeConfig::new(17, 16, 16),
            Err(VolumeConfigError::TooManyChunks { .. })
        ));
        assert!(matches!(
            VolumeConfig::new(32, 32, 32),
            Err(VolumeConfigError::TooManyChunks { .. })
        ));
    }

    #[test]
    fn chunk_mutation_changes_the_mesh() {
        let mut volume = sample_volume();
        let before = volume.mesh().checksum();
        volume.set_voxel(VolumePos::new(60, 60, 60), 4).unwrap();
        assert_ne!(before, volume.mesh().checksum());
    }
}
